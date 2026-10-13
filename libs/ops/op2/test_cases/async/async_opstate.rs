// Copyright 2018-2026 the Deno authors. MIT license.

#![deny(warnings)]
ende_ops_compile_test_runner::prelude!();

use std::cell::RefCell;
use std::rc::Rc;

use ende_core::OpState;

#[op2]
pub async fn op_async_opstate(
  state: Rc<RefCell<OpState>>,
) -> std::io::Result<i32> {
  Ok(*state.borrow().borrow::<i32>())
}
