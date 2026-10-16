export {};

declare global {
  /**
   * ```ts
   * import { assertEquals } from "@ende/assert/equals";
   * import "./mod.js";
   *
   * assertEquals(myFunction(1, 2), 3);
   * ```
   */
  export function myFunction(a: number, b: number): number;
}
