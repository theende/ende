try {
  Deno.readTextFileSync("./ende.json");
  console.log("loaded");
} catch (err) {
  console.log(err instanceof Deno.errors.NotFound);
}

try {
  Deno.readTextFileSync("data.txt");
  console.log("loaded");
} catch (err) {
  console.log(err instanceof Deno.errors.NotFound);
}
