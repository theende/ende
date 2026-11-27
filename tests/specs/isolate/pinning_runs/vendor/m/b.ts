export const manifest = { permissions: { env: true } };
export function both() {
  const r: unknown[] = [];
  try {
    r.push(Ende.readTextFileSync("data/a.txt").trim());
  } catch (e) {
    r.push((e as Error).name);
  }
  try {
    r.push(typeof Ende.env.get("HOME"));
  } catch (e) {
    r.push((e as Error).name);
  }
  return r.join(" | ");
}
