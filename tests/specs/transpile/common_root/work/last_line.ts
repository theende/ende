// Usage: last_line.ts <file>
//
// Prints the text after a file's last newline, so a trailing newline prints
// an empty line.
const text = Deno.readTextFileSync(Deno.args[0]);
console.log(text.slice(text.lastIndexOf("\n") + 1));
