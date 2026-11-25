// Removes the directory named by the first argument, recursively.
try {
  Deno.removeSync(Deno.args[0], { recursive: true });
  console.log("removed");
} catch (e) {
  console.log((e as Error).constructor.name);
}
