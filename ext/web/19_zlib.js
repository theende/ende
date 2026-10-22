// Copyright 2026 the Ende authors. MIT license.

// @ts-check
/// <reference path="../../core/lib.deno_core.d.ts" />
/// <reference path="./internal.d.ts" />

(function () {
const { core, primordials } = __bootstrap;
const {
  op_zlib_deflate,
  op_zlib_inflater_new,
  op_zlib_inflater_push,
} = core.ops;
const {
  Float64Array,
  NumberIsInteger,
  NumberIsSafeInteger,
  NumberMAX_SAFE_INTEGER,
  RangeError,
} = primordials;

const webidl = core.loadExtScript("ext:ende_webidl/00_webidl.js");

const convertFormat = webidl.createEnumConverter("ZlibFormat", [
  "deflate",
  "deflate-raw",
  "gzip",
]);

// [consumed, done], written by op_zlib_inflater_push.
const pushResult = new Float64Array(2);

class Inflater {
  #inflater;

  constructor(options = { __proto__: null }) {
    const prefix = "Failed to construct 'Inflater'";
    const format = options.format === undefined
      ? "deflate"
      : convertFormat(options.format, prefix, "format");
    const { maxOutputBytes } = options;
    if (
      maxOutputBytes !== undefined &&
      !(NumberIsSafeInteger(maxOutputBytes) && maxOutputBytes >= 0)
    ) {
      throw new RangeError(
        `${prefix}: maxOutputBytes must be a non-negative integer, received ${maxOutputBytes}`,
      );
    }
    this.#inflater = op_zlib_inflater_new(
      format,
      maxOutputBytes ?? NumberMAX_SAFE_INTEGER,
    );
  }

  push(chunk) {
    chunk = webidl.converters.BufferSource(
      chunk,
      "Failed to execute 'push' on 'Inflater'",
      "Argument 1",
    );
    const data = op_zlib_inflater_push(this.#inflater, chunk, pushResult);
    return { consumed: pushResult[0], data, done: pushResult[1] === 1 };
  }
}

function deflateSync(data, options = { __proto__: null }) {
  const prefix = "Failed to execute 'deflateSync'";
  data = webidl.converters.BufferSource(data, prefix, "Argument 1");
  const format = options.format === undefined
    ? "deflate"
    : convertFormat(options.format, prefix, "format");
  const level = options.level ?? 6;
  if (!NumberIsInteger(level) || level < 0 || level > 9) {
    throw new RangeError(
      `${prefix}: level must be an integer from 0 to 9, received ${level}`,
    );
  }
  return op_zlib_deflate(data, format, level);
}

return { Inflater, deflateSync };
})();
