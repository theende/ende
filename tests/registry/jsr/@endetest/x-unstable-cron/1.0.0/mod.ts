// Tiny JSR script used by `deno x` spec tests to verify that
// `--unstable-*` flags are forwarded into the spawned `deno run` invocation.
const ac = new AbortController();
const done = Deno.cron("x", "*/5 * * * *", { signal: ac.signal }, () => {});
ac.abort();
await done;
console.log("ok:", 42);
