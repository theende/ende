import { write } from "./vendor/writer/mod.ts";
import { show } from "./show.ts";

// The app never loads code from a folder a sandbox may write, whatever the
// file is called or whether it has an extension at all.
const code = 'export const evil = "ran code a sandbox wrote";';
for (const name of ["tool.ts", "helper", "x.txt"]) {
  await show(`write ${name}`, () => write(`out/${name}`, code));
}
for (const name of ["tool.ts", "helper", "x.txt"]) {
  await show(`load ${name}`, async () => (await import(`./out/${name}`)).evil);
}
// A file system that folds case also folds the long s: `c.tſ` is
// `c.ts` there.
if (Ende.build.os === "darwin") {
  await show("write c.tſ", () => write("out/c.tſ", code));
  await show("load c.ts", async () => (await import("./out/c.ts")).evil);
}
