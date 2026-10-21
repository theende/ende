export function add(a: number, b: number): number {
  return a + b;
}

export class Counter {
  count = 0;
}

export function makeCounter(): Counter {
  return new Counter();
}
