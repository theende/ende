// Test helper, run with --no-config: changes the pinned hash of a remote
// module and of a JSR package, as if their code had changed.
const lock = JSON.parse(Deno.readTextFileSync("ende.lock"));
const remote = "https://localhost:5545/subdir/print_hello.ts";
if (!lock.remote[remote]) throw new Error(`${remote} not pinned`);
lock.remote[remote] = "0".repeat(64);
const jsr = lock.jsr["@endetest/add@1.0.0"];
if (!jsr) throw new Error("@endetest/add@1.0.0 not pinned");
jsr.integrity = "0".repeat(64);
Deno.writeTextFileSync("ende.lock", JSON.stringify(lock, null, 2));
