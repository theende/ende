import { read } from "./vendor/none/mod.ts";

try {
  await read("secret.txt");
} catch (e) {
  console.log((e as Error).message);
}
Ende.bench("call", async () => {
  try {
    await read("data/a.txt");
  } catch {
    // denied, as above
  }
});
