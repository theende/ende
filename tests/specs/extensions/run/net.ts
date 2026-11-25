import { status } from "extension:@acme/net";

console.log(await status("http://localhost:4545/run/002_hello.ts"));
try {
  await fetch("http://localhost:4545/run/002_hello.ts");
} catch (e) {
  console.log((e as Error).name);
}
