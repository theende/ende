import { ctor, hasOwn, proto } from "extension:@sec/errors";
for (const call of [hasOwn, ctor, proto]) {
  try {
    await call();
  } catch (e) {
    console.log(e instanceof Error, (e as Error).name, (e as Error).message);
  }
}
setTimeout(() => console.log("host still alive"), 50);
