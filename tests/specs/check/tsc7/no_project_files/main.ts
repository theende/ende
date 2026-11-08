import * as remote from "http://localhost:4545/subdir/type_error.ts";
import { add } from "jsr:@endetest/add@1";

console.log(remote.a, add(1, 2));
