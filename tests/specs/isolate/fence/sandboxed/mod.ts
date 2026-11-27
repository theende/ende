const code = 'export const evil = "ran code a sandbox wrote";';

function attempt(label: string, call: () => void): string {
  try {
    call();
    return `${label}: ok`;
  } catch (e) {
    return `${label}: ${(e as Error).name}`;
  }
}

// Replaces its write grant's folder with one spelled differently.
export function swapCase(): string[] {
  return [
    attempt("remove out", () => Ende.removeSync("out", { recursive: true })),
    attempt("mkdir OUT", () => Ende.mkdirSync("OUT")),
    attempt("write OUT/x.ts", () => Ende.writeTextFileSync("OUT/x.ts", code)),
  ];
}

// `linked` starts as a symlink to `store`: writes through it, then
// replaces it with a folder of its own.
export function relink(): string[] {
  return [
    attempt(
      "write linked/y.ts",
      () => Ende.writeTextFileSync("linked/y.ts", code),
    ),
    attempt("remove linked", () => Ende.removeSync("linked")),
    attempt("mkdir linked", () => Ende.mkdirSync("linked")),
    attempt(
      "write linked/x.ts",
      () => Ende.writeTextFileSync("linked/x.ts", code),
    ),
  ];
}

export function writeThroughLink(): string {
  Ende.writeTextFileSync("link/x.ts", code);
  return "written";
}

export function ping(): string {
  return "pong";
}
