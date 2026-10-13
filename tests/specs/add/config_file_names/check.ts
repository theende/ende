for (const file of ["ende.json", "deno.json", "ende.lock", "deno.lock"]) {
  let text: string | undefined;
  try {
    text = Deno.readTextFileSync(file);
  } catch {
    // not found
  }
  if (text == null) {
    console.log(`${file}: false`);
  } else if (file.endsWith(".json")) {
    const hasDep = text.includes("@endetest/add");
    console.log(`${file}: true (${hasDep ? "has dep" : "no dep"})`);
  } else {
    console.log(`${file}: true`);
  }
}
