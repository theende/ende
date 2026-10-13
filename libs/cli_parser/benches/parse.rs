// Copyright 2018-2026 the Deno authors. MIT license.
//! Microbenchmarks for the hand-written CLI parser, isolated from runtime
//! startup so we can see the actual per-invocation parse cost.

use std::hint::black_box;

use criterion::BatchSize;
use criterion::Criterion;
use criterion::criterion_group;
use criterion::criterion_main;
use ende_cli_parser::convert::flags_from_vec;
use ende_cli_parser::defs::ENDE_ROOT;
use ende_cli_parser::parse;

fn svec(args: &[&str]) -> Vec<String> {
  args.iter().map(|s| s.to_string()).collect()
}

fn benches(c: &mut Criterion) {
  let bare = svec(&["ende", "run", "script.ts"]);
  let with_flags = svec(&[
    "ende",
    "run",
    "-A",
    "--no-check",
    "script.ts",
    "arg1",
    "arg2",
  ]);
  let complex = svec(&[
    "ende",
    "run",
    "--allow-read=/a,/b",
    "--allow-net=example.com",
    "--no-check",
    "--unstable-sloppy-imports",
    "--v8-flags=--max-old-space-size=4096",
    "--import-map=import_map.json",
    "--node-modules-dir=auto",
    "--reload",
    "script.ts",
    "arg1",
    "arg2",
  ]);
  let bare_repl = svec(&["ende"]);

  // Full parse + convert into `Flags` (the production entry point).
  let mut g = c.benchmark_group("flags_from_vec");
  for (name, input) in [
    ("bare_run", &bare),
    ("run_with_flags", &with_flags),
    ("run_complex", &complex),
    ("no_args_repl", &bare_repl),
  ] {
    g.bench_function(name, |b| {
      b.iter_batched(
        || input.clone(),
        |args| black_box(flags_from_vec(black_box(args))),
        BatchSize::SmallInput,
      );
    });
  }
  g.finish();

  // Raw `parse` only (walk the static tables into a ParseResult), no convert.
  let mut g = c.benchmark_group("parse_only");
  for (name, input) in [("bare_run", &bare), ("run_with_flags", &with_flags)] {
    g.bench_function(name, |b| {
      b.iter(|| black_box(parse(&ENDE_ROOT, black_box(input))));
    });
  }
  g.finish();
}

criterion_group!(cli_parser, benches);
criterion_main!(cli_parser);
