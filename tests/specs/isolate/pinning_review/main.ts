import { home, readData } from "./vendor/m/mod.ts";

for (const call of [readData, home]) {
  try {
    console.log(await call());
  } catch (error) {
    console.log(`${(error as Error).name}: ${(error as Error).message}`);
  }
}
