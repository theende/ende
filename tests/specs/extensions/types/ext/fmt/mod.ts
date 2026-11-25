export const manifest = { permissions: [] };

export function shout(text: string): string {
  return text.toUpperCase();
}

export async function len(text: string): Promise<number> {
  return text.length;
}

// A type error inside the extension: it's the extension's own problem and
// isn't reported when the app is checked.
function internal(): number {
  return "not a number";
}
internal;
