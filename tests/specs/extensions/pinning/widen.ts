// App code trying to approve a change to its own extensions by rewriting
// the files that declare and pin them.
const files = Deno.args.length ? Deno.args : ["ende.json", "ende.lock"];
for (const file of files) {
  try {
    let text = "";
    try {
      text = Deno.readTextFileSync(file);
    } catch {
      // a new file
    }
    Deno.writeTextFileSync(file, text);
    console.log(`wrote ${file}`);
  } catch (e) {
    console.log(`${(e as Error).constructor.name}: ${(e as Error).message}`);
  }
}
