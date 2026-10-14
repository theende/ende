// Copyright 2018-2026 the Deno authors. MIT license.

import { assert, assertEquals } from "./test_util.ts";

Deno.test(function version() {
  // Ende's product version is a ChronVer date: YYYY.MM.DD[.N]
  assert(/^\d{4}\.\d{2}\.\d{2}(\.[1-9]\d*)?$/.test(Ende.version.ende));
  assertEquals(
    Ende.version.ende,
    Deno.readTextFileSync(
      new URL("../../cli/lib/version.txt", import.meta.url),
    ).trim(),
  );
  assert(/^\d+\.\d+\.\d+/.test(Deno.version.v8));
  assertEquals(Deno.version.typescript, "6.0.3");
});
