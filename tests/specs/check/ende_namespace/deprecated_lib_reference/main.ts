/// <reference no-default-lib="true" />
/// <reference lib="deno.ns" />
/// <reference lib="deno.window" />
/// <reference lib="esnext" />

const text: string = await Deno.readTextFile("main.ts");
const conn: Ende.Conn | undefined = undefined;
console.log(text, conn);
