/**
 * A snippet in a sandboxed module's docs would run in the app, so
 * `ende test --doc` refuses it.
 *
 * ```ts
 * console.log("DOC SNIPPET RAN", Ende.readTextFileSync("secret.txt"));
 * ```
 */
export function f(): number {
  return 1;
}
