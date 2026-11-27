export function shout(text: string): string {
  return text.toUpperCase();
}

export const VERSION = "1.0.0";

export class Widget {}

export interface Options {
  loud: boolean;
}

// The package's own type error: its author's to fix, so checking the app
// doesn't report it.
const broken: number = "not a number";
export function brokenLength(): number {
  return broken;
}
