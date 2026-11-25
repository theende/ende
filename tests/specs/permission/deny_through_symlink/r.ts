// Reads the file named by the first argument and prints the outcome.
try {
  Deno.readTextFileSync(Deno.args[0]);
  console.log("read");
} catch (e) {
  console.log((e as Error).constructor.name);
}
