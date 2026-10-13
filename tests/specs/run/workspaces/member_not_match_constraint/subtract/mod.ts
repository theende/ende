import { add } from "@endetest/add";

export function subtract(a: number, b: number): number {
  return add(a, -b);
}
