// Resolves each map's `sources` against the map's own URL, as a debugger
// would, and checks that it lands on the original file.
const maps = [
  ["dist/main.js.map", "../src/main.ts"],
  ["dist/lib/greet.js.map", "../src/lib/greet.ts"],
];
for (const [map, source] of maps) {
  const { file, sources } = JSON.parse(Deno.readTextFileSync(map));
  const resolved = new URL(sources[0], new URL(map, import.meta.url));
  const expected = new URL(source, import.meta.url);
  console.log(
    file,
    Deno.realPathSync(resolved) === Deno.realPathSync(expected),
  );
}
