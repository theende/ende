// `sum()` is renamed to `add()` in 1.0.0, so this only type-checks against
// the 0.2.x versions published before the cutoff.
import { sum } from "jsr:@endetest/add";

console.log(sum(1, 2));
