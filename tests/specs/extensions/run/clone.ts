import { add } from "extension:@acme/math";

try {
  // @ts-expect-error a function can't cross to the extension
  await add(() => 1, 2);
} catch (e) {
  console.log((e as Error).name);
}
