// A literal import, in a module the app reaches only by a computed import.
export async function go(): Promise<string> {
  const { late } = await import("../vendor/m/late.ts");
  return await late();
}
