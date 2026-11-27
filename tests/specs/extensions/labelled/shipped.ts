import { read } from "extension:@acme/shipped";

for (const path of ["own.txt", "data/secret.txt"]) {
  try {
    console.log(`${path}: ${await read(path)}`);
  } catch (e) {
    console.log(`${path}: ${(e as Error).name}: ${(e as Error).message}`);
  }
}
