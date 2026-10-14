// Same directory `std::env::temp_dir()` uses for the code cache file.
function tmpdir(): string {
  if (Deno.build.os === "windows") {
    return Deno.env.get("TMP") ?? Deno.env.get("TEMP") ??
      Deno.env.get("USERPROFILE") ?? "C:\\Windows";
  }
  return (Deno.env.get("TMPDIR") ?? "/tmp").replace(/\/$/, "");
}

// cleanup the code cache file from a previous run
try {
  if (Deno.build.os === "windows") {
    Deno.removeSync(tmpdir() + "\\ende-compile-using_code_cache.exe.cache");
  } else {
    Deno.removeSync(tmpdir() + "/ende-compile-using_code_cache.cache");
  }
} catch {
}
