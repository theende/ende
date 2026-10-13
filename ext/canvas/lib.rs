// Copyright 2018-2026 the Deno authors. MIT license.

use ende_core::OpState;
use ende_core::op2;
use ende_core::v8;

mod bitmaprenderer;
pub mod byow;
mod canvas;

ende_core::extension!(
  ende_canvas,
  deps = [ende_webidl, ende_web, ende_webgpu],
  ops = [op_init_canvas],
  objects = [
    bitmaprenderer::ImageBitmapRenderingContext,
    canvas::OffscreenCanvas,
    byow::UnsafeWindowSurface,
  ],
  lazy_loaded_esm = ["01_canvas.js"],
  lazy_loaded_js = ["02_surface.js"],
);

#[op2(fast)]
pub fn op_init_canvas(
  state: &mut OpState,
  scope: &mut v8::PinScope<'_, '_>,
  blob: v8::Local<v8::Value>,
) {
  state.put(canvas::BlobHandle(v8::Global::new(scope, blob.cast())));
}
