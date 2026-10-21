import { PUNCT } from "../config.ts";

export { PUNCT };

export function greet(name: string): string {
  return `Hello, ${name}${PUNCT}`;
}
