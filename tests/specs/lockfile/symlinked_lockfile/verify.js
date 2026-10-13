// `deno add` must write through the symlink: the `ende.lock` symlink should be
// preserved and the real target file should receive the update, instead of the
// symlink being clobbered with a fresh regular file.
console.log("ende.lock is symlink:", Deno.lstatSync("ende.lock").isSymlink);
const target = Deno.readTextFileSync("real/ende.lock");
console.log("target updated:", target.includes("@denotest/add"));
