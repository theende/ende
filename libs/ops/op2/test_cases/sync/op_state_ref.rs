// Copyright 2018-2026 the Deno authors. MIT license.

#![deny(warnings)]
ende_ops_compile_test_runner::prelude!();

// Test w/ import pollution
#[allow(unused)]
use std::borrow::Borrow;
#[allow(unused)]
use std::borrow::BorrowMut;

use ende_core::OpState;
use ende_core::v8;

#[op2(fast)]
fn op_state_ref(_state: &OpState) {}

#[op2(fast)]
fn op_state_mut(_state: &mut OpState) {}

#[op2(fast)]
fn op_state_and_v8_local(
  _state: &mut OpState,
  _callback: v8::Local<v8::Function>,
) {
}
