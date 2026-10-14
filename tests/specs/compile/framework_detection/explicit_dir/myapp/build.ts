// Emits a Fresh 2 style `_fresh/server.js`. It reads from `static/` to prove
// the include paths were rebased against the detected app directory
// (./myapp) rather than the caller's cwd.
await Deno.mkdir("_fresh", { recursive: true });
await Deno.writeTextFile(
  "_fresh/server.js",
  `const html = await Deno.readTextFile(
  new URL("../static/index.html", import.meta.url),
);
console.log("explicit dir server started");
console.log(html.trim());
Deno.exit(0);
export default { fetch() {} };
`,
);
