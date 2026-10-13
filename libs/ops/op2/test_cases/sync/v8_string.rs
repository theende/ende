// Copyright 2018-2026 the Deno authors. MIT license.

#![deny(warnings)]
ende_ops_compile_test_runner::prelude!();

use ende_core::v8;

#[op2]
fn op_v8_string<'a>(
  _str1: &v8::String,
  _str2: v8::Local<v8::String>,
) -> v8::Local<'a, v8::String> {
  unimplemented!()
}
