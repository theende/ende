// A new version of the jsr package, or a changed vendored copy of the
// https module: either way, a manifest that asks for more.
const [what] = Ende.args;
if (what === "jsr") {
  const config = Ende.readTextFileSync("ende.json");
  Ende.writeTextFileSync("ende.json", config.replace("@1.0.0", "@1.1.0"));
} else if (what === "https") {
  const find = (dir: string): string | undefined => {
    for (const entry of Ende.readDirSync(dir)) {
      const path = `${dir}/${entry.name}`;
      if (entry.isFile && entry.name === "remote_ext.ts") return path;
      if (entry.isDirectory) {
        const found = find(path);
        if (found) return found;
      }
    }
  };
  const path = find("vendor");
  if (!path) throw new Error("remote_ext.ts not vendored");
  Ende.writeTextFileSync(
    path,
    Ende.readTextFileSync(path).replace(
      "permissions: []",
      'permissions: ["env"]',
    ),
  );
}
