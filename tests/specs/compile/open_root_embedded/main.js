// Embedded files have no directory to hold a handle to, so openRoot refuses
// them clearly.
const dataDir = import.meta.dirname + "/data";
try {
  Deno.openRootSync(dataDir);
  console.log("opened");
} catch (e) {
  console.log(`${e.constructor.name}: ${e.message}`);
}
