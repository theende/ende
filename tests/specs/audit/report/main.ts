import { add } from "jsr:@endetest/add@1";
import { sum } from "jsr:@endetest/add@0.2";
import { printHello } from "https://localhost:5545/subdir/print_hello.ts";
import { answer } from "https://localhost:5545/run/extension_lockfile/remote_ext.ts";
import { tool } from "./vendor/tool/mod.ts";

console.log(add(1, 2), sum(1, 2), answer(), tool());
printHello();
