export const manifest = { permissions: [] };
function named(name: string): Error {
  const error = new Error(`named ${name}`);
  error.name = name;
  return error;
}
export function hasOwn(): never {
  throw named("hasOwnProperty");
}
export function ctor(): never {
  throw named("constructor");
}
export function proto(): never {
  throw named("__proto__");
}
