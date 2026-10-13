import { ignored } from "./ignored.ts";

export function used(condition: boolean): boolean {
  // ende-coverage-ignore
  if (condition) return false;

  // ende-coverage-ignore-start
  if (condition) {
    return false;
  }
  // ende-coverage-ignore-stop

  // deno-coverage-ignore-start
  if (condition) {
    return false;
  }
  // deno-coverage-ignore-stop

  return ignored();
}
