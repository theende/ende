export const manifest = { permissions: [] };
export function home(): string {
  try {
    return String(Deno.env.get("HOME") !== undefined);
  } catch (e) {
    return (e as Error).name;
  }
}
