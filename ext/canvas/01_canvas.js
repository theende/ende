// Copyright 2018-2026 the Deno authors. MIT license.

import { core, primordials } from "ext:core/mod.js";
import {
  ImageBitmapRenderingContext,
  OffscreenCanvas,
  op_init_canvas,
  op_offscreen_canvas_receive,
  op_offscreen_canvas_transfer,
} from "ext:core/ops";

const {
  ObjectDefineProperty,
  ObjectPrototypeIsPrototypeOf,
  SymbolToStringTag,
} = primordials;
const { Blob } = core.loadExtScript("ext:ende_web/09_file.js");
const { DOMException, DOMExceptionPrototype } = core.loadExtScript(
  "ext:ende_web/01_dom_exception.js",
);
// transferToImageBitmap() creates ImageBitmaps, whose structured clone and
// transfer steps are defined by the image module.
core.createLazyLoader("ext:ende_image/01_image.js")();

// OffscreenCanvas is [Transferable] (but not [Serializable], see below).
const OffscreenCanvasPrototype = OffscreenCanvas.prototype;
ObjectDefineProperty(OffscreenCanvasPrototype, core.hostObjectBrand, {
  __proto__: null,
  value: "OffscreenCanvas",
});
core.registerTransferableResource(
  "OffscreenCanvas",
  (canvas) => {
    try {
      return op_offscreen_canvas_transfer(canvas);
    } catch (error) {
      if (ObjectPrototypeIsPrototypeOf(DOMExceptionPrototype, error)) {
        throw error;
      }
      // Detached.
      throw new DOMException(error.message, "DataCloneError");
    }
  },
  (rid) => op_offscreen_canvas_receive(rid),
);

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
