// The package asks for read access only: env stays denied although the
// rule grants it.
export const manifest = { permissions: { read: true } };

export function readData(): string {
  return Ende.readTextFileSync("data/a.txt").trim();
}

export function home(): string | undefined {
  return Ende.env.get("HOME");
}
