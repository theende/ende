// Exporting `then` makes a module namespace look like a promise.
export function then(_resolve: unknown): void {}
export function hello(): string {
  return "hello";
}
