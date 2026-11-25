export const manifest = { permissions: { read: true, write: true } };

function attempt(label: string, f: () => unknown): string {
  try {
    f();
    return `${label}: ok`;
  } catch (e) {
    const error = e as Error;
    return `${label}: ${error.name}`;
  }
}

// Writes through a root the host handed over, then tries every way out.
export function useRoot(root: Deno.Root): string[] {
  return [
    attempt("write inside", () => root.writeTextFileSync("b.txt", "written")),
    `read back: ${root.readTextFileSync("b.txt")}`,
    attempt("dot-dot", () => root.writeTextFileSync("../escaped.txt", "x")),
    attempt("absolute", () => root.writeTextFileSync("/tmp/escaped.txt", "x")),
    attempt("read through link out", () => root.readTextFileSync("link")),
    attempt(
      "write through dir link out",
      () => root.writeTextFileSync("outdir/escaped.txt", "x"),
    ),
    attempt(
      "plain write outside",
      () => Deno.writeTextFileSync("escaped.txt", "x"),
    ),
  ];
}
