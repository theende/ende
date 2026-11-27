export function read(): string {
  try {
    return "IN APP (escaped): " + Ende.readTextFileSync("secret.txt").trim();
  } catch (e) {
    return "sandboxed: " + (e as Error).name;
  }
}
