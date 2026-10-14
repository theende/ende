// Copyright 2026 the Ende authors. MIT license.

// Run with: deno test --config tools/deno.json tools/release/chronver_test.ts

import { assert, assertEquals, assertThrows } from "@std/assert";
import {
  compareChronVer,
  isNewerRelease,
  nextReleaseVersion,
  parseChronVer,
  releaseTagError,
} from "./chronver.ts";

Deno.test("parseChronVer accepts dates with an optional same-day number", () => {
  assertEquals(parseChronVer("2026.10.14"), {
    year: 2026,
    month: 10,
    day: 14,
    n: undefined,
  });
  assertEquals(parseChronVer("v2026.10.14.2"), {
    year: 2026,
    month: 10,
    day: 14,
    n: 2,
  });
  assertEquals(parseChronVer("2028.02.29")?.day, 29);
});

Deno.test("parseChronVer rejects anything else", () => {
  for (
    const text of [
      "2.9.7",
      "v2.9.7",
      "2026.1.14",
      "2026.10.4",
      "2026.13.01",
      "2026.02.29",
      "2026.04.31",
      "2026.10.14.0",
      "2026.10.14.01",
      "2026.10.14.1.1",
      "2026.10.14-rc",
      " 2026.10.14",
    ]
  ) {
    assertEquals(parseChronVer(text), undefined, text);
  }
});

Deno.test("compareChronVer orders by date, then same-day number", () => {
  const ordered = [
    "2026.10.13",
    "2026.10.14",
    "2026.10.14.1",
    "2026.10.14.2",
    "2026.10.14.10",
    "2026.11.01",
    "2027.01.01",
  ].map((v) => parseChronVer(v)!);
  for (let i = 1; i < ordered.length; i++) {
    assert(compareChronVer(ordered[i - 1], ordered[i]) < 0, `${i}`);
    assert(compareChronVer(ordered[i], ordered[i - 1]) > 0, `${i}`);
  }
  assertEquals(
    compareChronVer(
      parseChronVer("v2026.10.14")!,
      parseChronVer("2026.10.14")!,
    ),
    0,
  );
});

Deno.test("isNewerRelease treats SemVer releases as older", () => {
  assert(isNewerRelease("v2026.10.14.1", "v2026.10.14"));
  assert(!isNewerRelease("v2026.10.14", "v2026.10.14"));
  assert(!isNewerRelease("v2026.10.13", "2026.10.14"));
  // releases from before Ende used ChronVer
  assert(isNewerRelease("v2026.10.14", "v2.9.7"));
  assert(isNewerRelease("2026.10.14", "garbage"));
  assertThrows(() => isNewerRelease("v2.9.8", "v2.9.7"), Error, "ChronVer");
});

Deno.test("nextReleaseVersion uses today's UTC date", () => {
  const tags = new Set<string>();
  // 23:30 on Oct 13 in UTC-7 is already Oct 14 in UTC
  const now = new Date("2026-10-13T23:30:00-07:00");
  assertEquals(nextReleaseVersion(now, tags), "2026.10.14");
  assertEquals(
    nextReleaseVersion(new Date("2027-01-05T00:00:00Z"), tags),
    "2027.01.05",
  );
});

Deno.test("nextReleaseVersion numbers additional releases on the same day", () => {
  const now = new Date("2026-10-14T12:00:00Z");
  assertEquals(
    nextReleaseVersion(now, new Set(["v2026.10.13", "v2026.10.13.1"])),
    "2026.10.14",
  );
  assertEquals(
    nextReleaseVersion(now, new Set(["v2026.10.14"])),
    "2026.10.14.1",
  );
  assertEquals(
    nextReleaseVersion(
      now,
      new Set(["v2026.10.14", "v2026.10.14.1", "v2026.10.14.2"]),
    ),
    "2026.10.14.3",
  );
});

Deno.test("releaseTagError accepts v plus the product version", () => {
  assertEquals(releaseTagError("v2026.10.14", "2026.10.14"), undefined);
  assertEquals(releaseTagError("v2026.10.14.1", "2026.10.14.1"), undefined);
});

Deno.test("releaseTagError rejects tags that aren't ChronVer", () => {
  for (const tag of ["2026.10.14", "v2.1.0", "v2026.10.14-rc1", "primary"]) {
    assert(releaseTagError(tag, "2026.10.14")?.includes("not a ChronVer"));
  }
});

Deno.test("releaseTagError rejects tags that don't match version.txt", () => {
  assert(
    releaseTagError("v2026.10.14.1", "2026.10.14")?.includes("does not match"),
  );
});
