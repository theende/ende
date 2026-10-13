// Copyright 2018-2026 the Deno authors. MIT license.
import { assert, assertEquals } from "./test_util.ts";

function descriptorsMatch(): string | undefined {
  const ende = Object.getOwnPropertyDescriptor(globalThis, "Ende");
  const deno = Object.getOwnPropertyDescriptor(globalThis, "Deno");
  if (ende === undefined) return "globalThis.Ende is not defined";
  if (deno === undefined) return "globalThis.Deno is not defined";
  if (ende.value !== deno.value) return "Deno and Ende are different objects";
  for (const key of ["enumerable", "writable", "configurable"] as const) {
    if (ende[key] !== deno[key]) {
      return `descriptor '${key}' differs: Ende=${ende[key]} Deno=${deno[key]}`;
    }
  }
  if ("get" in ende || "get" in deno) return "unexpected accessor descriptor";
  return undefined;
}

Deno.test(function endeGlobalIsDenoAlias() {
  assert(globalThis.Ende === globalThis.Deno);
  assert(Deno === Ende);
});

Deno.test(function endeGlobalDescriptorMatchesDeno() {
  assertEquals(descriptorsMatch(), undefined);
});

Deno.test(async function endeGlobalAliasInWorker() {
  const workerSource = encodeURIComponent(`
    const ende = Object.getOwnPropertyDescriptor(globalThis, "Ende");
    const deno = Object.getOwnPropertyDescriptor(globalThis, "Deno");
    postMessage({
      same: Deno === Ende,
      ende: ende && {
        enumerable: ende.enumerable,
        writable: ende.writable,
        configurable: ende.configurable,
      },
      deno: deno && {
        enumerable: deno.enumerable,
        writable: deno.writable,
        configurable: deno.configurable,
      },
      version: Ende.version.ende,
      hasDenoKey: "deno" in Ende.version,
    });
    close();
  `);
  const worker = new Worker(`data:application/javascript,${workerSource}`, {
    type: "module",
  });
  const { promise, resolve, reject } = Promise.withResolvers<
    Record<string, unknown>
  >();
  worker.onmessage = (e) => resolve(e.data);
  worker.onerror = (e) => reject(e.error ?? e.message);
  try {
    const data = await promise;
    assertEquals(data.same, true);
    assert(data.ende !== undefined);
    assertEquals(data.deno, data.ende);
    assert(/^\d+\.\d+\.\d+/.test(data.version as string));
    assertEquals(data.hasDenoKey, false);
  } finally {
    worker.terminate();
  }
});

Deno.test(function endeVersionKey() {
  assert(/^\d+\.\d+\.\d+/.test(Ende.version.ende));
  assertEquals("deno" in Ende.version, false);
  assertEquals(Deno.version, Ende.version);
});

Deno.test(function endeUserAgent() {
  assert(
    navigator.userAgent.startsWith("Ende/"),
    `unexpected user agent: ${navigator.userAgent}`,
  );
});
