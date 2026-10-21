import { add } from "./util.ts";
import type { Point } from "./types.ts";
export * from "./types.ts";
export type { Point as Coordinates } from "./types.ts";

const point: Point = { x: 1, y: 2 };
console.log(add(point.x, point.y));

const { double } = await import("./dynamic.ts");
console.log(double(21));
