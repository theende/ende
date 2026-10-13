for (const file of ["project/ende.json", "project/deno.json"]) {
  let exists = false;
  try {
    exists = Deno.statSync(file).isFile;
  } catch {
    // not found
  }
  console.log(`${file}: ${exists}`);
}
