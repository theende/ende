// Copyright 2018-2026 the Deno authors. MIT license.

#![deny(warnings)]
ende_ops_compile_test_runner::prelude!();

#[op2(fast)]
fn op_string_owned(#[string] s: String) -> u32 {
  s.len() as _
}
