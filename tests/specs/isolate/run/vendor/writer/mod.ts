// Writes modules into its own folder and imports them: its own code may
// load, but code it writes can't reach outside the folder.
export async function importWritten(name: string, code: string) {
  Ende.writeTextFileSync(new URL(name, import.meta.url), code);
  try {
    const module = await import(`./${name}`);
    return JSON.stringify(module);
  } catch (e) {
    return `${(e as Error).name}: ${(e as Error).message}`;
  }
}
