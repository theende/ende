// Copyright 2026 the Ende authors. MIT license.
const scope = import.meta.url.slice(-7) === "#worker" ? "worker" : "main";

console.log(
  scope,
  typeof Ende.openKv,
  typeof Ende.Kv,
  typeof Ende.KvU64,
  typeof Ende.KvListIterator,
  typeof Ende.AtomicOperation,
  "openKv" in Ende,
);

if (scope === "worker") {
  postMessage("done");
} else {
  const worker = new Worker(`${import.meta.url}#worker`, { type: "module" });
  worker.onmessage = () => Ende.exit(0);
}
