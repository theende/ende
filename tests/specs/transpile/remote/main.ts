import { Foo } from "jsr:@endetest/subset-type-graph@0.1.0";
import { describe } from "./describe.ts";

export const foo = new Foo();

export function make() {
  return new Foo();
}

export const value: number = describe(foo);
