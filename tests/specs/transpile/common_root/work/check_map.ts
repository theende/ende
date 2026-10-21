// Usage: check_map.ts <emitted file> <original source>
//
// Reads the source map for an emitted file (`x.js.map`, `x.js` with an
// inline map, or `-` for an inline map on `ende transpile`'s stdout),
// resolves `sources[0]` against the map's URL as a debugger would, and prints
// `file`, `sources[0]`, whether the map holds a `file://` URL and whether the
// resolved source is the original file.
const [emitted, source] = Deno.args;
// Steps run from this directory.
const cwd = new URL("./", import.meta.url);

let mapText: string;
let mapUrl: URL;
if (emitted === "-") {
  // stdout has no location, so the map is relative to the cwd.
  const { stdout } = new Deno.Command(Deno.execPath(), {
    args: ["transpile", source, "--source-map", "inline"],
  }).outputSync();
  mapText = inlineMap(new TextDecoder().decode(stdout));
  mapUrl = cwd;
} else {
  const text = Deno.readTextFileSync(emitted);
  mapText = emitted.endsWith(".map") ? text : inlineMap(text);
  mapUrl = new URL(emitted, cwd);
}

const map = JSON.parse(mapText);
const resolved = new URL(map.sources[0], mapUrl);
console.log(
  map.file,
  map.sources[0],
  mapText.includes("file://"),
  Deno.realPathSync(resolved) === Deno.realPathSync(new URL(source, cwd)),
);

function inlineMap(js: string): string {
  const prefix = "//# sourceMappingURL=data:application/json;base64,";
  const line = js.split("\n").find((l) => l.startsWith(prefix))!;
  return atob(line.slice(prefix.length));
}
