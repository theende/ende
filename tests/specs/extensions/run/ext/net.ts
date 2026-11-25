export const manifest = { permissions: { net: ["localhost:4545"] } };

export async function status(url: string): Promise<number> {
  const response = await fetch(url);
  await response.body?.cancel();
  return response.status;
}
