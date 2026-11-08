// Clearing the last abort listener of a timeout signal must stop its timer
// from keeping the process alive.
const start = performance.now();
const signal = AbortSignal.timeout(10_000);
signal.onabort = () => {};
signal.onabort = null;
globalThis.addEventListener("unload", () => {
  console.log(
    performance.now() - start < 5_000
      ? "exited before the timeout"
      : "waited for the timeout",
  );
});
