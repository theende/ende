// Copyright 2018-2026 the Deno authors. MIT license.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use deno_error::JsErrorBox;
use denokv_proto::Database;
use ende_core::OpState;

#[async_trait(?Send)]
pub trait DatabaseHandler {
  type DB: Database + 'static;

  async fn open(
    &self,
    state: Rc<RefCell<OpState>>,
    path: Option<String>,
  ) -> Result<Self::DB, JsErrorBox>;
}
