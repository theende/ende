import { tmpdir } from "node:os";

// cleanup the code cache file from a previous run
try {
  if (Deno.build.os === "windows") {
    Deno.removeSync(tmpdir() + "\\ende-compile-using_code_cache.exe.cache");
  } else {
    Deno.removeSync(tmpdir() + "/ende-compile-using_code_cache.cache");
  }
} catch {
}
