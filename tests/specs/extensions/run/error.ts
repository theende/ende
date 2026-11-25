import { boom } from "extension:@acme/math";

try {
  await boom();
} catch (e) {
  const error = e as Error;
  console.log(`${error.name}: ${error.message}`);
  console.log(error.stack?.includes("ext/math.ts"));
}
