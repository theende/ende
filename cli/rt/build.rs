// Copyright 2018-2026 the Deno authors. MIT license.

fn main() {
  // Skip building from docs.rs.
  if std::env::var_os("DOCS_RS").is_some() {
    return;
  }

  ende_runtime::ende_webgpu::print_linker_flags("endert");
}
