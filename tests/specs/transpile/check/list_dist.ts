// Prints every file written to dist, or says that nothing was written.
function list(dir: string): string[] {
  return [...Deno.readDirSync(dir)].flatMap((entry) => {
    const path = `${dir}/${entry.name}`;
    return entry.isDirectory ? list(path) : [path];
  });
}
try {
  console.log(list("dist").sort().join("\n"));
} catch (error) {
  if (!(error instanceof Deno.errors.NotFound)) throw error;
  console.log("no dist");
}
