// Copyright 2018-2026 the Deno authors. MIT license.

#![deny(warnings)]
ende_ops_compile_test_runner::prelude!();

use std::cell::RefCell;
use std::rc::Rc;

use ende_core::OpState;

#[op2(fast)]
fn op_state_rc(_state: Rc<RefCell<OpState>>) {}
