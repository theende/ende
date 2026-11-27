// A sandbox granted read and write.
export function read(path: string): string {
  return Ende.readTextFileSync(path);
}

export function write(path: string): string {
  Ende.writeTextFileSync(path, "changed by the sandbox");
  return "written";
}
