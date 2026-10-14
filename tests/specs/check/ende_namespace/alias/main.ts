const a: string = await Ende.readTextFile(import.meta.filename!);
const b: string = await Deno.readTextFile(import.meta.filename!);

let c: Deno.Conn | undefined;
let d: Ende.Conn | undefined;
c = d;
d = c;

// unstable augmentations of the namespace are visible through both names
let cs: Deno.CronSchedule | undefined;
let cs2: Ende.CronSchedule | undefined;
cs = cs2;
cs2 = cs;

const ns: typeof Deno = Ende;
const version: string = Deno.version.ende;
const fromWindow: typeof Ende = window.Deno;

console.log(a, b, c, d, cs, cs2, ns, version, fromWindow);
