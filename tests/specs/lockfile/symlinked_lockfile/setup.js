// Place the real lockfile in a sibling directory and symlink `ende.lock` to
// it, like the Docker setup in https://github.com/denoland/deno/issues/28153
// where the real lockfile lives on a separate volume.
Deno.mkdirSync("real");
Deno.writeTextFileSync("real/ende.lock", '{\n  "version": "5"\n}\n');
// The symlink target is stored verbatim in the reparse point on Windows, and a
// forward slash there is not resolvable (ERROR_INVALID_NAME), so use the
// platform separator for the relative target.
const sep = Deno.build.os === "windows" ? "\\" : "/";
Deno.symlinkSync(`real${sep}ende.lock`, "ende.lock");
