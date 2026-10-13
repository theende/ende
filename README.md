# Ende

Ende is a JavaScript, TypeScript, and WebAssembly runtime with secure defaults
and a great developer experience. It's built on [V8](https://v8.dev/),
[Rust](https://www.rust-lang.org/), and [Tokio](https://tokio.rs/).

Learn more about the Ende runtime
[in the documentation](https://docs.ende.sh/runtime/manual).

## Installation

Install the Ende runtime on your system using one of the commands below.

Shell (Mac, Linux):

```sh
curl -fsSL https://ende.sh/install.sh | sh
```

PowerShell (Windows):

```powershell
irm https://ende.sh/install.ps1 | iex
```

### Build and install from source

Complete instructions for building Ende from source can be found
[here](.github/CONTRIBUTING.md#building-from-source).

## Your first Ende program

Ende can be used for many different applications, but is most commonly used to
build web servers. Create a file called `server.ts` and include the following
TypeScript code:

```ts
Ende.serve((_req: Request) => {
  return new Response("Hello, world!");
});
```

Run your server with the following command:

```sh
ende run --allow-net server.ts
```

This should start a local web server on
[http://localhost:8000](http://localhost:8000).

Learn more about writing and running Ende programs
[in the docs](https://docs.ende.sh/runtime/manual).

## Coming from Deno

Ende is a fork of [Deno](https://github.com/denoland/deno), and we're grateful
to the Deno authors for the work it builds on. Existing Deno projects keep
working: the `Deno` global is a permanent alias of `Ende`, and `deno.json`,
`deno.lock` and `DENO_*` environment variables are still read (with a
deprecation warning) when their `ende.json`, `ende.lock` and `ENDE_*`
counterparts are absent.

## Additional resources

- **[Ende Docs](https://docs.ende.sh)**: guides and reference docs for the Ende
  runtime.
- **[Standard Library](https://jsr.io/@std)**: common utilities for JavaScript
  and TypeScript programs.
- **[JSR](https://jsr.io/)**: The open-source package registry for modern
  JavaScript and TypeScript

## Contributing

We appreciate your help! To contribute, please read our
[contributing instructions](.github/CONTRIBUTING.md).
