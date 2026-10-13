const text: string = await Deno.readTextFile("main.ts");
const conn: Ende.Conn | undefined = undefined;
console.log(text, conn);
