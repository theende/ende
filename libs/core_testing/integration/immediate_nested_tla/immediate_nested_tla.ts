// Copyright 2018-2026 the Deno authors. MIT license.
// An immediate queued from inside an immediate runs in the next event loop
// iteration. It must keep the loop alive and must not be reported as a
// stalled top-level await.
console.log("start");
await new Promise((resolve) => {
  setImmediate(() => {
    console.log("outer");
    setImmediate(() => {
      console.log("inner");
      resolve(null);
    });
  });
});
console.log("done");
