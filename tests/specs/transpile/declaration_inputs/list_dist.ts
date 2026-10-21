// Prints every file written to dist, so a test can tell that nothing was
// emitted for the declaration inputs.
function list(dir: string): string[] {
  return [...Deno.readDirSync(dir)].flatMap((entry) => {
    const path = `${dir}/${entry.name}`;
    return entry.isDirectory ? list(path) : [path];
  });
}
console.log(list("dist").sort().join("\n"));
