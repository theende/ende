// Test helper, run with --no-config: edits a fixture.
const [what] = Deno.args;
const replace = (path: string, from: string, to: string) => {
  const text = Deno.readTextFileSync(path);
  if (!text.includes(from)) throw new Error(`${from} not in ${path}`);
  Deno.writeTextFileSync(path, text.replace(from, to));
};
switch (what) {
  case "code":
    replace("ext/fmt/mod.ts", ".trim());", '.trim() + "!");');
    break;
  case "helper":
    replace("ext/fmt/helper.ts", "toUpperCase()", "toLowerCase()");
    break;
  case "manifest":
    replace("ext/fmt/mod.ts", "{ read: true }", "{ read: true, env: true }");
    break;
  case "grant":
    replace("ende.json", '["./data"]', '["./data", "./other"]');
    break;
  case "remove":
    Deno.writeTextFileSync(
      "ende.json",
      JSON.stringify({ extensions: {} }),
    );
    break;
  case "drop_section": {
    // What an older Ende does when it rewrites the lockfile.
    const lock = JSON.parse(Deno.readTextFileSync("ende.lock"));
    if (!lock.extensions) throw new Error("no extensions section");
    delete lock.extensions;
    Deno.writeTextFileSync("ende.lock", JSON.stringify(lock, null, 2));
    break;
  }
  case "old_lockfile":
    Deno.writeTextFileSync("ende.lock", '{\n  "version": "4"\n}\n');
    break;
  case "remote_checksum": {
    const lock = JSON.parse(Deno.readTextFileSync("ende.lock"));
    const url = "https://localhost:5545/run/extension_lockfile/remote_ext.ts";
    if (!lock.remote?.[url]) throw new Error("remote not pinned");
    lock.remote[url] = "0".repeat(64);
    Deno.writeTextFileSync("ende.lock", JSON.stringify(lock, null, 2));
    break;
  }
  default:
    throw new Error(`unknown edit ${what}`);
}
