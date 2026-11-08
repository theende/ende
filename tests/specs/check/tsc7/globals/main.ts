// `Ende.readTextFile` returns `Promise<string>`: the first line checks, and
// the second shows the type isn't `any`.
const text: string = await Ende.readTextFile("data.txt");
const wrong: Promise<number> = Ende.readTextFile("data.txt");
console.log(text, wrong);
