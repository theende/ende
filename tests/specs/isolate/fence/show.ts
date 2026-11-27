export async function load(path: string): Promise<string> {
  try {
    return (await import(path)).evil;
  } catch (e) {
    return `refused: ${(e as Error).message.split("\n")[0]}`;
  }
}
