// A type-only import from jsr is erased from the JS but kept in the .d.ts.
import type { Foo } from "jsr:@endetest/subset-type-graph@0.1.0";

export function describe(foo: Foo): number {
  return foo.method();
}
