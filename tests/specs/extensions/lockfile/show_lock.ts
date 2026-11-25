// Prints what ende.lock pins about the extensions.
const lock = JSON.parse(Deno.readTextFileSync("ende.lock"));
for (const [name, entry] of Object.entries(lock.extensions ?? {})) {
  const e = entry as Record<string, unknown>;
  console.log(
    name,
    e.source,
    JSON.stringify(e.manifest),
    JSON.stringify(e.grant),
    typeof e.integrity,
  );
}
