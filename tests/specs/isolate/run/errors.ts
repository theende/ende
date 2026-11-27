import { fail } from "./vendor/none/mod.ts";

for (const kind of ["NotFound", "RangeError", "AbortError", "CustomError"]) {
  try {
    await fail(kind);
  } catch (e) {
    console.log(
      kind,
      (e as Error).name,
      (e as Error).message,
      e instanceof Ende.errors.NotFound,
      e instanceof RangeError,
      e instanceof DOMException,
    );
  }
}
