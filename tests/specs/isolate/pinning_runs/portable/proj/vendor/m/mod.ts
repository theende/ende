export const manifest = { permissions: { read: true } };

export function readShared(): string {
  return Ende.readTextFileSync("../shared/s.txt").trim();
}
