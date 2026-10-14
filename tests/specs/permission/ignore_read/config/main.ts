try {
  Deno.readTextFileSync("./ende.json");
  console.log("loaded");
} catch (err) {
  console.log(err instanceof Deno.errors.NotFound);
}

try {
  Deno.statSync("./ende.json");
  console.log("loaded");
} catch (err) {
  console.log(err instanceof Deno.errors.NotFound);
}

try {
  await Deno.stat("./ende.json");
  console.log("loaded");
} catch (err) {
  console.log(err instanceof Deno.errors.NotFound);
}
