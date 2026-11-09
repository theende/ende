// Copyright 2018-2026 the Deno authors. MIT license.

import { core, primordials } from "ext:core/mod.js";
import {
  ImageBitmapRenderingContext,
  OffscreenCanvas,
  op_init_canvas,
} from "ext:core/ops";

const { ObjectDefineProperty, SymbolToStringTag } = primordials;
const { Blob } = core.loadExtScript("ext:ende_web/09_file.js");

op_init_canvas(Blob);

ObjectDefineProperty(ImageBitmapRenderingContext.prototype, SymbolToStringTag, {
  __proto__: null,
  value: "ImageBitmapRenderingContext",
  writable: false,
  enumerable: false,
  configurable: true,
});
ObjectDefineProperty(OffscreenCanvas.prototype, SymbolToStringTag, {
  __proto__: null,
  value: "OffscreenCanvas",
  writable: false,
  enumerable: false,
  configurable: true,
});

// Not [Serializable] (OffscreenCanvas is only [Transferable]):
// structuredClone throws a DataCloneError.
{
  const { markNotSerializable } = core.loadExtScript(
    "ext:ende_web/13_message_port.js",
  );
  markNotSerializable(OffscreenCanvas.prototype);
  markNotSerializable(ImageBitmapRenderingContext.prototype);
}

export { ImageBitmapRenderingContext, OffscreenCanvas };
