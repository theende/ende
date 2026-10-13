// Copyright 2018-2026 the Deno authors. MIT license.
import { core } from "ext:core/mod.js";
const mod = core.loadExtScript(
  "ext:ende_node/internal/streams/duplex.js",
);

export const Duplex = mod.Duplex;
export default mod.default;
