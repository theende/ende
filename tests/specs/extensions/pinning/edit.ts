// Test helper, run with --no-config: edits a fixture.
const [what] = Deno.args;
const replace = (path: string, from: string, to: string) => {
  const text = Deno.readTextFileSync(path);
  if (!text.includes(from)) throw new Error(`${from} not in ${path}`);
  Deno.writeTextFileSync(path, text.replace(from, to));
};
const find = (dir: string, name: string): string | undefined => {
  for (const entry of Deno.readDirSync(dir)) {
    const path = `${dir}/${entry.name}`;
    if (entry.isFile && entry.name === name) return path;
    if (entry.isDirectory) {
      const found = find(path, name);
      if (found) return found;
    }
  }
};
switch (what) {
  case "payload":
    replace("ext/plug/payload.txt", "payload-v1", "payload-v2");
    break;
  case "plugin":
    replace("ext/plug/plugin.ts", "plugin-v1", "plugin-v2");
    break;
  case "vendor": {
    const path = find("vendor", "remote_ext.ts");
    if (!path) throw new Error("remote_ext.ts not vendored");
    replace(path, "return 42", "return 43");
    break;
  }
  default:
    throw new Error(`unknown edit ${what}`);
}
