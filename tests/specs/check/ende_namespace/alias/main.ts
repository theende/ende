const a: string = await Ende.readTextFile(import.meta.filename!);
const b: string = await Deno.readTextFile(import.meta.filename!);

let c: Deno.Conn | undefined;
let d: Ende.Conn | undefined;
c = d;
d = c;

// unstable augmentations of the namespace are visible through both names
let kv: Deno.Kv | undefined;
let kv2: Ende.Kv | undefined;
kv = kv2;
kv2 = kv;

const ns: typeof Deno = Ende;
const version: string = Deno.version.ende;
const fromWindow: typeof Ende = window.Deno;

console.log(a, b, c, d, kv, kv2, ns, version, fromWindow);
