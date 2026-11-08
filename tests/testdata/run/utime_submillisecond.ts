// Copyright 2018-2026 the Deno authors. MIT license.

const path = "file.txt";
const atime = Deno.args[1] === "date"
  ? new Date(1_600_000_000_765)
  : 1_600_000_000.7654321;
const mtime = Deno.args[1] === "date"
  ? new Date(1_700_000_000_123)
  : 1_700_000_000.1234567;
Deno.writeTextFileSync(path, "");

switch (Deno.args[0]) {
  case "deno_sync":
    Deno.utimeSync(path, atime, mtime);
    break;
  case "deno_async":
    await Deno.utime(path, atime, mtime);
    break;
  case "file_sync": {
    using file = Deno.openSync(path, { write: true });
    file.utimeSync(atime, mtime);
    break;
  }
  case "file_async": {
    using file = await Deno.open(path, { write: true });
    await file.utime(atime, mtime);
    break;
  }
  default:
    throw new Error(`Unknown API: ${Deno.args[0]}`);
}
