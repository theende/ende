for (const file of ["main.d.ts", "lib/consts.d.mts"]) {
  console.log(`== ${file}`);
  console.log(Deno.readTextFileSync(`dist/${file}`).trimEnd());
}
