import { add } from "./lib/util.tsx";
export type { Point } from "./lib/types.ts";
// Single quotes survive the rewrite.
// ende-fmt-ignore
export * from './lib/consts.mts';
export { LEGACY } from "./lib/legacy.cts";

export const sum: typeof import("./lib/util.tsx").add = add;
