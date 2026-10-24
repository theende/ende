// Copyright 2018-2026 the Deno authors. MIT license.

let expectedDepth: number | null = 4;
let expectedCompact: boolean | number = 3;

const customInspect: Ende.CustomInspect = (inspect, options) => {
  const depth: number | null = options.depth;
  const indentLevel: number = options.indentLevel;
  const forwardedOptions: Ende.InspectOptions = options;
  if (
    depth !== expectedDepth || indentLevel !== 0 ||
    options.compact !== expectedCompact
  ) {
    throw new Error("Unexpected custom inspection options");
  }
  return `Wrapper ${inspect({ value: 42 }, forwardedOptions)}`;
};

const value = { [Symbol.for("Ende.customInspect")]: customInspect };
if (Ende.inspect(value) !== "Wrapper { value: 42 }") {
  throw new Error("Custom inspection did not forward the options");
}

expectedDepth = null;
if (Ende.inspect(value, { depth: null }) !== "Wrapper { value: 42 }") {
  throw new Error("Custom inspection did not preserve unlimited depth");
}

expectedDepth = 4;
expectedCompact = 2;
if (Ende.inspect(value, { compact: 2 }) !== "Wrapper { value: 42 }") {
  throw new Error("Custom inspection did not preserve numeric compact options");
}
