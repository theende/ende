for (const file of ["main.d.ts", "describe.d.ts"]) {
  console.log(`== ${file}`);
  console.log(Deno.readTextFileSync(`dist/${file}`).trimEnd());
}
