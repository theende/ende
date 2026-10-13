// Copyright 2018-2026 the Deno authors. MIT license.

#![deny(warnings)]
ende_ops_compile_test_runner::prelude!();

use ende_core::JsBuffer;

#[op2]
#[buffer]
pub async fn op_async_v8_buffer(#[buffer] buf: JsBuffer) -> JsBuffer {
  buf
}
