// Another entry of the same package, without a manifest.
export function hasHome(): boolean {
  return Ende.env.get("HOME") !== undefined;
}
