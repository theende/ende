// Copyright 2018-2026 the Deno authors. MIT license.

use ende_core::JsBuffer;
use ende_core::OpState;
use ende_core::op2;

use super::TestData;

#[op2]
pub fn op_v8slice_store(
  state: &mut OpState,
  #[string] name: String,
  #[buffer] data: JsBuffer,
) {
  state.borrow_mut::<TestData>().insert(name, data);
}

#[op2]
#[buffer]
pub fn op_v8slice_clone(state: &OpState, #[string] name: String) -> Vec<u8> {
  state.borrow::<TestData>().get::<JsBuffer>(name).to_vec()
}
