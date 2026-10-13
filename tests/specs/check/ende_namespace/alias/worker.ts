/// <reference no-default-lib="true" />
/// <reference lib="ende.worker" />
/// <reference lib="esnext" />

const text: string = await Deno.readTextFile("data.txt");
let conn: Deno.Conn | undefined;
let conn2: Ende.Conn | undefined;
conn = conn2;
conn2 = conn;
const fromSelf: typeof Ende = self.Deno;
self.postMessage({ text, conn, conn2, same: fromSelf === self.Ende });
