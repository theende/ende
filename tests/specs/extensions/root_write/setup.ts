// Symlinks need unscoped write, so the links are made in a setup step: one
// inside output/ to a file outside it, and one to output/'s parent.
Deno.symlinkSync("../secret.txt", "output/link");
Deno.symlinkSync("..", "output/outdir");
