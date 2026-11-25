// Writes the file named by the first argument and prints the outcome.
try {
  Deno.writeTextFileSync(Deno.args[0], "x");
  console.log("wrote");
} catch (e) {
  console.log((e as Error).constructor.name);
}
