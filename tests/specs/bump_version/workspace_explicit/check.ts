// Verifies post-bump file contents.
console.log("--- a/ende.json ---");
console.log(Deno.readTextFileSync("a/ende.json").trim());
console.log("--- b/ende.json ---");
console.log(Deno.readTextFileSync("b/ende.json").trim());
console.log("--- import_map.json ---");
console.log(Deno.readTextFileSync("import_map.json").trim());
