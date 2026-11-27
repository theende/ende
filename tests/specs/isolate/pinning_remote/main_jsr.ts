import { hasHome, version } from "pkg";

console.log(await version());
try {
  console.log(await hasHome());
} catch (error) {
  console.log((error as Error).message);
}
