// Copyright 2018-2026 the Deno authors. MIT license.

use std::convert::Infallible;

use deno_error::JsErrorBox;
use deno_error::JsErrorClass;
use ende_resolver::EndeResolveError;
use ende_resolver::EndeResolveErrorKind;
use ende_runtime::ende_core::error::AnyError;
use ende_runtime::ende_core::error::CoreError;
use ende_runtime::ende_core::error::CoreErrorKind;

pub trait InfallibleResultExt<T> {
  fn unwrap_infallible(self) -> T;
}

impl<T> InfallibleResultExt<T> for Result<T, Infallible> {
  fn unwrap_infallible(self) -> T {
    match self {
      Ok(value) => value,
      Err(never) => match never {},
    }
  }
}

pub fn js_error_downcast_ref(
  err: &AnyError,
) -> Option<&ende_runtime::ende_core::error::JsError> {
  any_and_jserrorbox_downcast_ref(err)
    .or_else(|| {
      err
        .downcast_ref::<CoreError>()
        .and_then(|e| match e.as_kind() {
          CoreErrorKind::Js(e) => Some(e),
          _ => None,
        })
    })
    .map(|v| &**v)
}

pub fn any_and_jserrorbox_downcast_ref<
  E: std::error::Error + Send + Sync + 'static,
>(
  err: &AnyError,
) -> Option<&E> {
  err
    .downcast_ref::<E>()
    .or_else(|| {
      err
        .downcast_ref::<JsErrorBox>()
        .and_then(|e| e.get_ref().downcast_ref::<E>())
    })
    .or_else(|| {
      err
        .downcast_ref::<CoreError>()
        .and_then(|e| match e.as_kind() {
          CoreErrorKind::JsBox(e) => e.get_ref().downcast_ref::<E>(),
          _ => None,
        })
    })
}

pub fn downcast_ref_ende_resolve_error(
  err: &JsErrorBox,
) -> Option<&EndeResolveErrorKind> {
  err
    .get_ref()
    .downcast_ref::<EndeResolveError>()
    .map(|e| e.as_kind())
    .or_else(|| err.get_ref().downcast_ref::<EndeResolveErrorKind>())
}
