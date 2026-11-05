function attempt(label: string, fn: () => unknown) {
  try {
    fn();
    console.log(`${label}: ok`);
  } catch (err) {
    console.log(`${label}: ${(err as Error).name}`);
  }
}

attempt("read inside", () => Ende.readTextFileSync("inside/a.txt"));
attempt("read outside", () => Ende.readTextFileSync("outside/b.txt"));
attempt("env", () => Ende.env.get("HOME"));
