// This package has a type error, which only --check=all reports.
import { Foo } from "jsr:@endetest/subset-type-graph-invalid@0.1.0";

export const foo = new Foo();
