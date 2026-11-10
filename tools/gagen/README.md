# gagen (vendored)

A copy of [`jsr:@david/gagen@0.3.1`](https://jsr.io/@david/gagen/0.3.1) by David
Sherret (MIT, see [LICENSE](./LICENSE)), the library the
`.github/workflows/*.ts` generators use. It's vendored because every published
version imports `node:` built-ins, which Ende doesn't support, and CI runs the
generator checks in `tools/lint.js` under `ende`.

The files are the package's `src/*.ts`, without its tests. Changes from 0.3.1:

- `workflow.ts`: `node:fs` and `node:process` replaced with
  `Deno.readTextFileSync`, `Deno.writeTextFileSync`, `Deno.exit` and
  `Deno.args`.
- `pin.ts`: `execSync` from `node:child_process` replaced with
  `new Deno.Command("git").outputSync()`. It still throws when git exits
  non-zero, but no longer has a 30-second timeout. git only runs when resolving
  a pin that isn't already in the generated file.

Nothing else is changed, so a later version can be diffed against this copy. For
the same reason, `tools/lint.js` doesn't lint it and
`tools/copyright_checker.js` skips it.
