import { len, shout } from "extension:@acme/fmt";
import { double } from "extension:@acme/jsr";

// A sync export is a Promise through the proxy.
const shouted: Promise<string> = shout("hi");
const length: number = await len("four");
const doubled: number = await double(21);
console.log(await shouted, length, doubled);
