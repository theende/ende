// Confirms --dry-run did not modify files.
console.log("--- a/ende.json ---");
console.log(Deno.readTextFileSync("a/ende.json").trim());
console.log("--- b/ende.json ---");
console.log(Deno.readTextFileSync("b/ende.json").trim());
