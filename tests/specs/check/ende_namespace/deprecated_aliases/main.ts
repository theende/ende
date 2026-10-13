// Copyright 2026 the Ende authors. MIT license.
/// <reference lib="ende.desktop" />

// `DenoTest` is a deprecated alias of `EndeTest`.
const t1: Ende.EndeTest = Ende.test;
const t2: Deno.DenoTest = Deno.test;
const t3: Ende.DenoTest = t1;
const t4: Deno.EndeTest = t2;

// The Worker option `deno` is a deprecated alias of `ende`.
const w1: WorkerOptions = { ende: { permissions: "none" } };
const w2: WorkerOptions = { deno: { permissions: "none" } };
const w3: WorkerOptions = {
  ende: { permissions: "inherit" },
  deno: { permissions: "none" },
};

// The `openDevtools()` option `deno` is a deprecated alias of `ende`.
const d1: Ende.OpenDevtoolsOptions = { renderer: false, ende: true };
const d2: Ende.OpenDevtoolsOptions = { renderer: false, deno: true };

console.log(t1, t2, t3, t4, w1, w2, w3, d1, d2);
