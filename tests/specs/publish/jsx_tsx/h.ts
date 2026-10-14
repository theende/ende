// Copyright 2026 the Ende authors. MIT license.

declare global {
  // deno-lint-ignore no-namespace
  namespace JSX {
    interface IntrinsicElements {
      [name: string]: unknown;
    }
  }
}

export function h(tag: string, _props: unknown, ...children: string[]): string {
  return `<${tag}>${children.join("")}</${tag}>`;
}
