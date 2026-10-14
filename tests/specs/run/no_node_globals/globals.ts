// Copyright 2026 the Ende authors. MIT license.
export const nodeGlobals = [
  typeof (globalThis as Record<string, unknown>).process,
  typeof (globalThis as Record<string, unknown>).Buffer,
  typeof (globalThis as Record<string, unknown>).global,
  typeof (globalThis as Record<string, unknown>).setImmediate,
  typeof (globalThis as Record<string, unknown>).clearImmediate,
].join(" ");
