for (const name of ["ende.lock", "deno.lock"]) {
  let text: string | undefined;
  try {
    text = Deno.readTextFileSync(name);
  } catch {
    // not found
  }
  if (text == null) {
    console.log(`${name}: false`);
  } else {
    const hasRemote = text.includes("print_hello.ts");
    console.log(`${name}: true (${hasRemote ? "has remote" : "no remote"})`);
  }
}
