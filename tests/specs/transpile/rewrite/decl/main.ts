import { add, makeCounter } from "./lib/util.tsx";
export type { Point } from "./lib/types.ts";
// Single quotes survive the rewrite.
// ende-fmt-ignore
export * from './lib/consts.mts';

export const sum: typeof import("./lib/util.tsx").add = add;
// The inferred type references util.tsx, which tsc prints itself.
export const counter = makeCounter();
