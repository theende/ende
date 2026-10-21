// Copyright 2018-2026 the Deno authors. MIT license.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use deno_ast::EmitOptions;
use deno_ast::MediaType;
use deno_ast::SourceMapOption;
use deno_ast::SourceRange;
use deno_ast::SourceRangedForSpanned;
use deno_ast::TextChange;
use deno_ast::TranspileModuleOptions;
use deno_ast::swc::ast;
use deno_ast::swc::ecma_visit::VisitMut;
use deno_ast::swc::ecma_visit::VisitMutWith;
use deno_graph::GraphKind;
use deno_graph::ModuleGraph;
use deno_terminal::colors;
use ende_core::ModuleSpecifier;
use ende_core::anyhow;
use ende_core::anyhow::Context;
use ende_core::error::AnyError;
use ende_resolver::emit::patch_public_decorator_access_has;
use sys_traits::PathsInErrorsExt;

use crate::args::Flags;
use crate::args::SourceMapMode;
use crate::args::TranspileFlags;
use crate::args::TypeCheckMode;
use crate::factory::CliFactory;
use crate::util::fs::canonicalize_path;
use crate::util::fs::canonicalize_path_maybe_not_exists;

pub async fn transpile(
  flags: Arc<Flags>,
  transpile_flags: TranspileFlags,
) -> Result<(), AnyError> {
  let files = &transpile_flags.files;

  if files.is_empty() {
    anyhow::bail!("No input files specified");
  }

  // Declaration files are skipped below, so they don't count as inputs.
  let input_count = files
    .iter()
    .filter(|file| !MediaType::from_path(Path::new(file)).is_declaration())
    .count();

  if input_count > 1 && transpile_flags.output.is_some() {
    anyhow::bail!(
      "Cannot use --output with multiple input files. Use --outdir instead."
    );
  }

  if transpile_flags.declaration
    && transpile_flags.output.is_none()
    && transpile_flags.output_dir.is_none()
  {
    anyhow::bail!(
      "Cannot use --declaration without --output or --outdir. Declaration files must be written to disk."
    );
  }

  let is_stdout_mode = input_count == 1
    && transpile_flags.output.is_none()
    && transpile_flags.output_dir.is_none();

  if is_stdout_mode
    && matches!(transpile_flags.source_map, SourceMapMode::Separate)
  {
    anyhow::bail!(
      "Cannot use --source-map separate when outputting to stdout. Use --output or --outdir, or use --source-map inline instead."
    );
  }

  let factory = CliFactory::from_flags(flags.clone());
  let cli_options = factory.cli_options()?;
  let cwd = cli_options.initial_cwd();
  let real_sys = factory.sys();
  let sys = real_sys.with_paths_in_errors();

  let source_map_option = match transpile_flags.source_map {
    SourceMapMode::None => SourceMapOption::None,
    SourceMapMode::Inline => SourceMapOption::Inline,
    SourceMapMode::Separate => SourceMapOption::Separate,
  };

  // Resolve transpile options from ende.json compilerOptions
  let compiler_options_resolver = factory.compiler_options_resolver()?;

  // Collect file paths and specifiers
  let mut file_entries = Vec::new();
  // Declaration files have nothing to emit, but stay in the program that
  // --declaration type-checks so the ambient types they declare resolve.
  let mut declaration_entries = Vec::new();
  for file_path_str in files {
    let file_path = cwd.join(file_path_str);

    let media_type = MediaType::from_path(&file_path);
    if !media_type.is_emittable() && !media_type.is_declaration() {
      log::warn!(
        "{} Skipping {} (not a TypeScript/JSX/TSX file)",
        colors::yellow("Warning"),
        file_path.display()
      );
      continue;
    }

    // Resolve `..` and symlinks so a file names one output however it's
    // spelled on the command line.
    let file_path = canonicalize_path(&file_path)
      .with_context(|| format!("Failed to read {}", file_path.display()))?;

    let specifier =
      ModuleSpecifier::from_file_path(&file_path).map_err(|_| {
        anyhow::anyhow!("Invalid file path: {}", file_path.display())
      })?;

    if media_type.is_declaration() {
      log::info!(
        "{} {} (declaration file)",
        colors::green("Skip"),
        file_path.display()
      );
      declaration_entries.push((specifier, media_type));
    } else {
      file_entries.push((file_path, specifier, media_type));
    }
  }

  if file_entries.is_empty() {
    anyhow::bail!(
      "No emittable files found. Only TypeScript/JSX/TSX files can be transpiled."
    );
  }

  // Declaration inputs aren't emitted, so they don't widen the root.
  let out_dir = match &transpile_flags.output_dir {
    Some(dir) => {
      let inputs: Vec<&Path> = file_entries
        .iter()
        .map(|(path, _, _)| path.as_path())
        .collect();
      Some(OutDir {
        dir: cwd.join(dir),
        root: common_parent_dir(&inputs)?,
      })
    }
    None => None,
  };

  // Type-check before writing anything, so a failed check leaves no output
  // behind. Declaration emit type-checks, so it needs no separate pass.
  let type_check_mode = cli_options.type_check_mode();
  let declarations = if transpile_flags.declaration {
    emit_declarations(
      &factory,
      &transpile_flags,
      &file_entries,
      declaration_entries,
      out_dir.as_ref(),
      cwd,
      type_check_mode,
    )
    .await?
  } else {
    if type_check_mode.is_true() {
      let roots = file_entries
        .iter()
        .map(|(_, specifier, _)| specifier.clone())
        .chain(
          declaration_entries
            .into_iter()
            .map(|(specifier, _)| specifier),
        )
        .collect();
      factory
        .module_graph_creator()
        .await?
        .create_graph_and_maybe_check(roots)
        .await?;
    }
    Vec::new()
  };

  // Transpile each file (TS -> JS)
  for (file_path, specifier, media_type) in &file_entries {
    let source_bytes = sys
      .fs_read(file_path)
      .with_context(|| format!("Failed to read {}", file_path.display()))?;
    let source_code = String::from_utf8(source_bytes.into_owned())
      .with_context(|| format!("{} is not valid UTF-8", file_path.display()))?;

    // Rewrite relative specifiers in the AST rather than the output text so
    // the literals keep their spans and source maps stay accurate.
    let parsed_source = deno_ast::parse_module_with_post_process(
      deno_ast::ParseParams {
        specifier: specifier.clone(),
        text: source_code.into(),
        media_type: *media_type,
        capture_tokens: false,
        scope_analysis: false,
        maybe_syntax: None,
      },
      |mut module, _| {
        module.visit_mut_with(&mut RelativeSpecifierRewriter::default());
        module
      },
    )?;

    // Use transpile options from ende.json, with source map overridden by CLI flag
    let transpile_and_emit_options = compiler_options_resolver
      .for_specifier(specifier)
      .transpile_options()?;
    let transpile_options = &transpile_and_emit_options.transpile;

    // Compute the output path/filename so source_map_file references the
    // generated file (e.g. "out.js"), not the input (e.g. "main.ts").
    let maybe_output_path = if !is_stdout_mode {
      Some(compute_output_path(
        file_path,
        cwd,
        transpile_flags.output.as_deref(),
        out_dir.as_ref(),
        *media_type,
      ))
    } else {
      None
    };
    let source_map_filename = if let Some(output_path) = &maybe_output_path {
      output_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
    } else {
      // In stdout mode, use the input-derived JS filename
      let ext = js_extension_for_media_type(*media_type);
      file_path
        .with_extension(ext)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
    };

    // Make the map's `sources` relative to where it is read from: the
    // emitted file's directory (a separate map sits next to it), or the cwd
    // for stdout.
    let source_map_base = if matches!(source_map_option, SourceMapOption::None)
    {
      None
    } else {
      let map_dir = maybe_output_path
        .as_deref()
        .and_then(Path::parent)
        .unwrap_or(cwd);
      source_map_base(map_dir, file_path)?
    };

    // Always apply the CLI source map mode. The built-in emit defaults
    // have inlineSourceMap: true (for the runtime), but `ende transpile`
    // should produce clean output by default. Users must pass --source-map
    // explicitly to enable source maps.
    let emit_options = EmitOptions {
      source_map: source_map_option,
      source_map_base,
      source_map_file: Some(source_map_filename),
      ..transpile_and_emit_options.emit.clone()
    };

    let transpile_result = parsed_source.transpile(
      transpile_options,
      &TranspileModuleOptions { module_kind: None },
      &emit_options,
    )?;

    let mut emitted = transpile_result.into_source();
    patch_public_decorator_access_has(&mut emitted.text);
    // Skip the newline restoration pass when source maps are requested,
    // since inserting newlines would shift line numbers in the generated
    // output without a corresponding adjustment to the mappings.
    let emitted_text;
    let js_text = if matches!(source_map_option, SourceMapOption::None) {
      emitted_text = restore_block_comment_newlines(&emitted.text);
      &emitted_text
    } else {
      &emitted.text
    };
    let source_map_text = &emitted.source_map;

    // Determine output path
    if is_stdout_mode {
      // Single file, no output specified: print to stdout
      ende_print::drop_write_stdout(js_text.as_bytes());
    } else {
      let output_path = maybe_output_path.unwrap();

      // Ensure parent directory exists
      if let Some(parent) = output_path.parent() {
        sys.fs_create_dir_all(parent).with_context(|| {
          format!("Failed to create directory {}", parent.display())
        })?;
      }

      sys
        .fs_write(&output_path, js_text.as_bytes())
        .with_context(|| {
          format!("Failed to write {}", output_path.display())
        })?;
      log::info!("{} {}", colors::green("Emit"), output_path.display());

      // Write separate source map file
      if let Some(sm) = source_map_text
        && matches!(transpile_flags.source_map, SourceMapMode::Separate)
      {
        let ext = output_path
          .extension()
          .unwrap_or_default()
          .to_string_lossy();
        let map_path = output_path.with_extension(format!("{ext}.map"));
        sys
          .fs_write(&map_path, sm.as_bytes())
          .with_context(|| format!("Failed to write {}", map_path.display()))?;
        log::info!("{} {}", colors::green("Emit"), map_path.display());
      }
    }
  }

  // Write .d.ts declaration files if requested
  for (output_path, content) in &declarations {
    if let Some(parent) = output_path.parent() {
      sys.fs_create_dir_all(parent).with_context(|| {
        format!("Failed to create directory {}", parent.display())
      })?;
    }

    sys
      .fs_write(output_path, content.as_bytes())
      .with_context(|| format!("Failed to write {}", output_path.display()))?;
    log::info!("{} {}", colors::green("Emit"), output_path.display());
  }

  Ok(())
}

/// Runs tsc on the inputs and returns the declaration files to write with
/// their output paths. Fails on the type errors `type_check_mode` reports.
async fn emit_declarations(
  factory: &CliFactory,
  transpile_flags: &TranspileFlags,
  file_entries: &[(PathBuf, ModuleSpecifier, MediaType)],
  declaration_entries: Vec<(ModuleSpecifier, MediaType)>,
  out_dir: Option<&OutDir>,
  cwd: &Path,
  type_check_mode: TypeCheckMode,
) -> Result<Vec<(PathBuf, String)>, AnyError> {
  let cli_options = factory.cli_options()?;

  // Build a module graph for the input files
  let root_names: Vec<(ModuleSpecifier, MediaType)> = file_entries
    .iter()
    .map(|(_, specifier, media_type)| (specifier.clone(), *media_type))
    .chain(declaration_entries)
    .collect();

  let specifiers: Vec<ModuleSpecifier> =
    root_names.iter().map(|(s, _)| s.clone()).collect();

  let mut graph = ModuleGraph::new(GraphKind::All);
  let module_graph_builder = factory.module_graph_builder().await?;
  module_graph_builder
    .build_graph_roots(
      &mut graph,
      specifiers,
      crate::graph_util::BuildGraphOptions {
        is_dynamic: false,
        loader: None,
      },
    )
    .await?;

  // Validate that all files share the same compiler options scope, since
  // TSC type-checks all files in a single pass.
  let compiler_options_resolver = factory.compiler_options_resolver()?;
  let first_specifier = &root_names[0].0;
  let (first_key, _) =
    compiler_options_resolver.entry_for_specifier(first_specifier);
  for (specifier, _) in root_names.iter().skip(1) {
    let (key, _) = compiler_options_resolver.entry_for_specifier(specifier);
    if key != first_key {
      anyhow::bail!(
        "Cannot generate declarations for files with different compiler option scopes.\n\
         '{}' uses scope '{}' but '{}' uses scope '{}'.\n\
         Separate these files into different invocations.",
        first_specifier,
        first_key,
        specifier,
        key,
      );
    }
  }

  let type_checker = factory.type_checker().await?;
  let result = type_checker.emit_declarations(
    Arc::new(graph),
    root_names,
    cli_options.ts_type_lib_window(),
    type_check_mode,
  )?;

  if result.diagnostics.has_diagnostic() {
    anyhow::bail!("Type checking failed:\n{}", result.diagnostics);
  }

  // Determine the output directory for .d.ts files.
  // With -o, place .d.ts next to the output file.
  // With --outdir, use the outdir.
  // Otherwise, place next to the source file (handled by TSC path).
  let output_base_dir = if let Some(output) = &transpile_flags.output {
    let output_path = cwd.join(output);
    output_path.parent().map(|p| p.to_path_buf())
  } else {
    None
  };

  let mut declarations = Vec::with_capacity(result.emitted_files.len());
  for (file_name, content) in &result.emitted_files {
    // The file names from TSC are specifier-based, convert to output paths
    let output_path =
      resolve_dts_output_path(file_name, out_dir, output_base_dir.as_deref())?;

    let content = if MediaType::from_path(&output_path).is_declaration() {
      rewrite_declaration_specifiers(&output_path, content)?
    } else {
      content.clone()
    };

    declarations.push((output_path, content));
  }

  if result.emitted_files.is_empty() {
    log::warn!(
      "{} No declaration files were emitted",
      colors::yellow("Warning")
    );
  }

  Ok(declarations)
}

fn resolve_dts_output_path(
  tsc_file_name: &str,
  out_dir: Option<&OutDir>,
  output_base_dir: Option<&Path>,
) -> Result<PathBuf, AnyError> {
  // TSC emits file names like "file:///path/to/file.d.ts"
  let path = if let Ok(specifier) = ModuleSpecifier::parse(tsc_file_name) {
    deno_path_util::url_to_file_path(&specifier).with_context(|| {
      format!("Cannot convert specifier to path: {tsc_file_name}")
    })?
  } else {
    PathBuf::from(tsc_file_name)
  };

  if let Some(out_dir) = out_dir {
    // tsc also emits declarations for local files the inputs import.
    out_dir.output_path(&path).ok_or_else(|| {
      anyhow::anyhow!(
        "Declaration file {} is outside {}, the common parent directory of the input files. Pass its source file as an input too.",
        path.display(),
        out_dir.root.display()
      )
    })
  } else if let Some(base_dir) = output_base_dir {
    // -o: place .d.ts next to the output file
    let file_name = path
      .file_name()
      .ok_or_else(|| anyhow::anyhow!("Invalid declaration file path"))?;
    Ok(base_dir.join(file_name))
  } else {
    // No output specified: place next to source file
    Ok(path)
  }
}

/// SWC's code generator always writes a single space after a block
/// comment's closing `*/`, regardless of whether the original source had a
/// newline. For multi-line block comments (typically JSDoc) this collapses
/// the comment onto the same line as the following statement. This function
/// walks the emitted source and replaces that single space with a newline
/// (plus the comment's own leading indentation) when the preceding block
/// comment spans multiple lines.
fn restore_block_comment_newlines(input: &str) -> String {
  let bytes = input.as_bytes();
  let n = bytes.len();
  let mut out: Vec<u8> = Vec::with_capacity(n);
  let mut i = 0;

  // Stack of brace depths at which template-literal substitutions started.
  // When `}` is reached at the top of the stack, we re-enter the template.
  let mut template_stack: Vec<u32> = Vec::new();
  let mut brace_depth: u32 = 0;

  // 0 = code, 1 = single-quote string, 2 = double-quote string,
  // 3 = template literal.
  let mut mode: u8 = 0;

  while i < n {
    let c = bytes[i];
    match mode {
      0 => match c {
        b'\'' => {
          out.push(c);
          mode = 1;
          i += 1;
        }
        b'"' => {
          out.push(c);
          mode = 2;
          i += 1;
        }
        b'`' => {
          out.push(c);
          mode = 3;
          i += 1;
        }
        b'{' => {
          brace_depth += 1;
          out.push(c);
          i += 1;
        }
        b'}' => {
          if template_stack.last() == Some(&brace_depth) {
            template_stack.pop();
            mode = 3;
          }
          brace_depth = brace_depth.saturating_sub(1);
          out.push(c);
          i += 1;
        }
        b'/' if i + 1 < n && bytes[i + 1] == b'/' => {
          // Line comment: copy through end of line.
          while i < n && bytes[i] != b'\n' {
            out.push(bytes[i]);
            i += 1;
          }
        }
        b'/' if i + 1 < n && bytes[i + 1] == b'*' => {
          // Block comment: copy through `*/`, then maybe restore newline.
          let start = i;
          out.push(bytes[i]);
          out.push(bytes[i + 1]);
          i += 2;
          let mut has_newline = false;
          while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
            if bytes[i] == b'\n' {
              has_newline = true;
            }
            out.push(bytes[i]);
            i += 1;
          }
          if i + 1 < n {
            out.push(b'*');
            out.push(b'/');
            i += 2;
          }
          // If multi-line and SWC emitted the trailing space, replace it
          // with a newline + the comment's own indentation. We only act
          // when the comment is indented by pure whitespace on its line;
          // otherwise leave the output untouched to avoid breaking layout.
          if has_newline && i < n && bytes[i] == b' ' {
            // Locate the start of the line containing the comment.
            let mut line_start = start;
            while line_start > 0 && bytes[line_start - 1] != b'\n' {
              line_start -= 1;
            }
            let indent = &bytes[line_start..start];
            if indent.iter().all(|&b| b == b' ' || b == b'\t') {
              out.push(b'\n');
              out.extend_from_slice(indent);
              i += 1; // skip the space SWC wrote
            }
          }
        }
        _ => {
          out.push(c);
          i += 1;
        }
      },
      1 | 2 => {
        let quote = if mode == 1 { b'\'' } else { b'"' };
        out.push(c);
        i += 1;
        if c == b'\\' && i < n {
          out.push(bytes[i]);
          i += 1;
        } else if c == quote {
          mode = 0;
        }
      }
      3 => {
        if c == b'\\' && i + 1 < n {
          out.push(c);
          out.push(bytes[i + 1]);
          i += 2;
        } else if c == b'`' {
          out.push(c);
          mode = 0;
          i += 1;
        } else if c == b'$' && i + 1 < n && bytes[i + 1] == b'{' {
          out.push(c);
          out.push(b'{');
          brace_depth += 1;
          template_stack.push(brace_depth);
          mode = 0;
          i += 2;
        } else {
          out.push(c);
          i += 1;
        }
      }
      _ => unreachable!(),
    }
  }

  // Safety: input is valid UTF-8 and we only branch on ASCII bytes; the
  // newlines and spaces we insert are ASCII. Multi-byte UTF-8 sequences are
  // copied byte-for-byte so the output remains valid UTF-8.
  String::from_utf8(out).expect("post-processed text must remain valid UTF-8")
}

/// Applies the JS emit's specifier rewrite to a declaration file tsc
/// emitted. Only the literals are replaced, so tsc's formatting is kept.
fn rewrite_declaration_specifiers(
  path: &Path,
  text: &str,
) -> Result<String, AnyError> {
  let specifier = ModuleSpecifier::from_file_path(path)
    .map_err(|_| anyhow::anyhow!("Invalid file path: {}", path.display()))?;
  let mut rewriter = RelativeSpecifierRewriter::default();
  let parsed_source = deno_ast::parse_module_with_post_process(
    deno_ast::ParseParams {
      specifier,
      text: text.into(),
      media_type: MediaType::from_path(path),
      capture_tokens: false,
      scope_analysis: false,
      maybe_syntax: None,
    },
    |mut module, _| {
      module.visit_mut_with(&mut rewriter);
      module
    },
  )?;
  let start = parsed_source.range().start;
  let changes = rewriter
    .rewritten
    .into_iter()
    .map(|(range, new_text)| TextChange {
      range: range.as_byte_range(start),
      new_text,
    })
    .collect();
  Ok(deno_ast::apply_text_changes(text, changes))
}

/// Points relative import specifiers at the files `ende transpile` emits
/// (`./util.ts` -> `./util.js`) so the output runs without the sources.
#[derive(Default)]
struct RelativeSpecifierRewriter {
  /// Original range and new source text of every rewritten literal.
  rewritten: Vec<(SourceRange, String)>,
}

impl RelativeSpecifierRewriter {
  fn rewrite(&mut self, str_lit: &mut ast::Str) {
    let Some(value) = str_lit.value.as_str() else {
      return;
    };
    let Some(new_value) = rewrite_relative_specifier(value) else {
      return;
    };
    // Keep the original quote style when the literal has no escapes.
    let raw = match &str_lit.raw {
      Some(raw) if &raw[1..raw.len() - 1] == value => {
        let quote = &raw[..1];
        format!("{quote}{new_value}{quote}")
      }
      _ => ende_core::serde_json::Value::from(new_value.as_str()).to_string(),
    };
    self.rewritten.push((str_lit.range(), raw.clone()));
    str_lit.raw = Some(raw.into());
    str_lit.value = new_value.into();
  }
}

impl VisitMut for RelativeSpecifierRewriter {
  fn visit_mut_import_decl(&mut self, node: &mut ast::ImportDecl) {
    self.rewrite(&mut node.src);
  }

  fn visit_mut_export_all(&mut self, node: &mut ast::ExportAll) {
    self.rewrite(&mut node.src);
  }

  fn visit_mut_named_export(&mut self, node: &mut ast::NamedExport) {
    if let Some(src) = &mut node.src {
      self.rewrite(src);
    }
  }

  fn visit_mut_ts_import_type(&mut self, node: &mut ast::TsImportType) {
    node.visit_mut_children_with(self);
    self.rewrite(&mut node.arg);
  }

  fn visit_mut_ts_external_module_ref(
    &mut self,
    node: &mut ast::TsExternalModuleRef,
  ) {
    self.rewrite(&mut node.expr);
  }

  fn visit_mut_call_expr(&mut self, node: &mut ast::CallExpr) {
    node.visit_mut_children_with(self);
    if matches!(node.callee, ast::Callee::Import(_))
      && let Some(arg) = node.args.first_mut()
      && let ast::Expr::Lit(ast::Lit::Str(str_lit)) = &mut *arg.expr
    {
      self.rewrite(str_lit);
    }
  }
}

/// Maps a `./` or `../` specifier for an emittable file to the name it is
/// emitted under, keeping any query string or fragment. Returns `None` for
/// anything else, including `.d.ts` files.
fn rewrite_relative_specifier(specifier: &str) -> Option<String> {
  if !specifier.starts_with("./") && !specifier.starts_with("../") {
    return None;
  }
  let path_end = specifier.find(['?', '#']).unwrap_or(specifier.len());
  let (path, suffix) = specifier.split_at(path_end);
  let media_type = MediaType::from_str(path);
  if !media_type.is_emittable() {
    return None;
  }
  let stem = &path[..path.rfind('.')?];
  let ext = js_extension_for_media_type(media_type);
  Some(format!("{stem}.{ext}{suffix}"))
}

fn js_extension_for_media_type(media_type: MediaType) -> &'static str {
  match media_type {
    MediaType::Mts => "mjs",
    MediaType::Cts => "cjs",
    _ => "js",
  }
}

/// Where `--outdir` writes: each file's path relative to `root`, the
/// inputs' common parent directory (like tsc's inferred `rootDir`), is
/// mirrored under `dir`.
struct OutDir {
  dir: PathBuf,
  root: PathBuf,
}

impl OutDir {
  /// Returns `None` when `path` isn't under `root`.
  fn output_path(&self, path: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(&self.root).ok()?;
    Some(self.dir.join(relative))
  }
}

/// Returns the deepest directory containing every path in `paths`.
fn common_parent_dir(paths: &[&Path]) -> Result<PathBuf, AnyError> {
  let first = paths[0];
  let mut common = first.parent().unwrap_or(first).to_path_buf();
  for path in &paths[1..] {
    while !path.starts_with(&common) {
      if !common.pop() {
        anyhow::bail!(
          "Input files {} and {} have no common parent directory (they are on different drives). Transpile them separately.",
          first.display(),
          path.display()
        );
      }
    }
  }
  Ok(common)
}

/// Returns the URL a source map's `sources` are made relative to: the
/// directory the map is read from. `None` when `source` is on another
/// Windows drive, which only its absolute URL can reach.
fn source_map_base(
  map_dir: &Path,
  source: &Path,
) -> Result<Option<ModuleSpecifier>, AnyError> {
  // Inputs are canonicalized, so the relative path is between real
  // locations.
  let map_dir = canonicalize_path_maybe_not_exists(map_dir)
    .with_context(|| format!("Failed to resolve {}", map_dir.display()))?;
  if map_dir.components().next() != source.components().next() {
    return Ok(None);
  }
  let base = ModuleSpecifier::from_directory_path(&map_dir).map_err(|_| {
    anyhow::anyhow!("Invalid directory path: {}", map_dir.display())
  })?;
  Ok(Some(base))
}

fn compute_output_path(
  input_path: &Path,
  cwd: &Path,
  output: Option<&str>,
  out_dir: Option<&OutDir>,
  media_type: MediaType,
) -> PathBuf {
  if let Some(output) = output {
    // Explicit output file
    return cwd.join(output);
  }

  let ext = js_extension_for_media_type(media_type);
  let js_filename = input_path.with_extension(ext);

  if let Some(out_dir) = out_dir {
    out_dir
      .output_path(&js_filename)
      .expect("inputs are under their common parent directory")
  } else {
    // Write alongside source file
    js_filename
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn transpile_rewrite_relative_specifier() {
    let cases = [
      ("./util.ts", Some("./util.js")),
      ("../component.tsx", Some("../component.js")),
      ("./view.jsx", Some("./view.js")),
      ("./lib.mts", Some("./lib.mjs")),
      ("./lib.cts", Some("./lib.cjs")),
      ("./dir/UPPER.TS", Some("./dir/UPPER.js")),
      ("./util.ts?v=1#top", Some("./util.js?v=1#top")),
      ("./util.ts#top", Some("./util.js#top")),
      ("./types.d.ts", None),
      ("./types.d.mts", None),
      ("./styles.d.css.ts", None),
      ("./util.js", None),
      ("./data.json", None),
      ("./dir", None),
      (".", None),
      ("util.ts", None),
      ("/abs/util.ts", None),
      ("file:///abs/util.ts", None),
      ("jsr:@std/path/join.ts", None),
      ("npm:pkg/util.ts", None),
      ("https://example.com/util.ts", None),
      ("data:text/typescript,export {}", None),
    ];
    for (specifier, expected) in cases {
      assert_eq!(
        rewrite_relative_specifier(specifier).as_deref(),
        expected,
        "{specifier}"
      );
    }
  }

  #[cfg(not(windows))]
  #[test]
  fn transpile_common_parent_dir() {
    let cases: [(&[&str], &str); 5] = [
      (&["/p/src/main.ts"], "/p/src"),
      (&["/p/src/main.ts", "/p/src/lib/x.ts"], "/p/src"),
      (&["/p/src/lib/x.ts", "/p/src/main.ts"], "/p/src"),
      (&["/p/src/a/x.ts", "/p/src/ab/y.ts"], "/p/src"),
      (&["/p/x.ts", "/q/y.ts"], "/"),
    ];
    for (paths, expected) in cases {
      let paths: Vec<&Path> = paths.iter().map(Path::new).collect();
      assert_eq!(
        common_parent_dir(&paths).unwrap(),
        PathBuf::from(expected),
        "{paths:?}"
      );
    }
  }

  #[cfg(windows)]
  #[test]
  fn transpile_common_parent_dir() {
    let paths = [Path::new("C:\\p\\main.ts"), Path::new("C:\\p\\lib\\x.ts")];
    assert_eq!(common_parent_dir(&paths).unwrap(), PathBuf::from("C:\\p"));
    let paths = [Path::new("C:\\p\\main.ts"), Path::new("D:\\p\\x.ts")];
    let err = common_parent_dir(&paths).unwrap_err().to_string();
    assert!(err.contains("no common parent directory"), "{err}");
  }

  #[cfg(windows)]
  #[test]
  fn transpile_source_map_base_other_drive() {
    let base =
      source_map_base(Path::new("C:\\out"), Path::new("D:\\src\\x.ts"));
    assert_eq!(base.unwrap(), None);
  }

  #[test]
  fn transpile_rewrite_declaration_specifiers() {
    let text = concat!(
      "/// <reference path=\"./ref.ts\" />\n",
      "export type { A } from \"./a.ts\";\n",
      "export * from './b.mts';\n",
      "export { c } from \"./\\u0063.cts\";\n",
      "export declare const d: typeof import(\"./d.tsx\").d;\n",
      "export type { E } from \"./e.d.ts\";\n",
      "export type { F } from \"jsr:@scope/f\";\n",
    );
    let expected = concat!(
      "/// <reference path=\"./ref.ts\" />\n",
      "export type { A } from \"./a.js\";\n",
      "export * from './b.mjs';\n",
      "export { c } from \"./c.cjs\";\n",
      "export declare const d: typeof import(\"./d.js\").d;\n",
      "export type { E } from \"./e.d.ts\";\n",
      "export type { F } from \"jsr:@scope/f\";\n",
    );
    let path = if cfg!(windows) {
      Path::new("C:\\out\\main.d.ts")
    } else {
      Path::new("/out/main.d.ts")
    };
    assert_eq!(
      rewrite_declaration_specifiers(path, text).unwrap(),
      expected
    );
  }
}
