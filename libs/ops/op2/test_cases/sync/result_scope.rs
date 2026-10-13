// Copyright 2018-2026 the Deno authors. MIT license.

#![deny(warnings)]
ende_ops_compile_test_runner::prelude!();

use deno_error::JsErrorBox;
use ende_core::v8;

#[op2(fast)]
pub fn op_void_with_result(
  _scope: &mut v8::PinScope,
) -> Result<(), JsErrorBox> {
  Ok(())
}
