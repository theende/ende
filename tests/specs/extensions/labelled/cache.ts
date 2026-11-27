import { write } from "extension:@acme/cache";

// A grant inside a protected folder doesn't open it: the vendor folder and
// the module cache (ENDE_DIR) stay the app's.
for (
  const path of [
    "vendor/jsr.io/evil.ts",
    "cache/gen/evil.js",
    ".git/hooks/pre-commit",
  ]
) {
  try {
    console.log(`${path}: ${await write(path)}`);
  } catch (e) {
    console.log(`${path}: ${(e as Error).name}: ${(e as Error).message}`);
  }
}
