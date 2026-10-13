// Copyright 2018-2026 the Deno authors. MIT license.
import { core } from "ext:core/mod.js";
const mod = core.loadExtScript("ext:ende_node/test/reporters.ts");

export const {
  dot,
  junit,
  lcov,
  spec,
  tap,
} = mod;

export default mod.default;
