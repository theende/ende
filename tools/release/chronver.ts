// Copyright 2026 the Ende authors. MIT license.

// Ende's product version is a ChronVer date, `YYYY.MM.DD[.N]`: the UTC day of
// the release, with `.N` (1 or more) for additional releases on the same day.
// `cli/lib/version.txt` holds it; `cli/lib/chronver.rs` is the Rust side.
// Release tags are `v<version>`.

export interface ChronVer {
  year: number;
  month: number;
  day: number;
  n: number | undefined;
}

const CHRONVER_RE = /^v?(\d{4})\.(\d{2})\.(\d{2})(?:\.([1-9]\d*))?$/;

/** The file holding the product version. */
export const PRODUCT_VERSION_FILE = new URL(
  "../../cli/lib/version.txt",
  import.meta.url,
);

export function readProductVersion(): string {
  return Deno.readTextFileSync(PRODUCT_VERSION_FILE).trim();
}

export function writeProductVersion(version: string) {
  if (parseChronVer(version) == null) {
    throw new Error(`Not a ChronVer version: ${version}`);
  }
  Deno.writeTextFileSync(PRODUCT_VERSION_FILE, version + "\n");
}

/** Parses `YYYY.MM.DD[.N]` with an optional leading `v`. */
export function parseChronVer(text: string): ChronVer | undefined {
  const match = CHRONVER_RE.exec(text);
  if (match == null) {
    return undefined;
  }
  const [year, month, day] = match.slice(1, 4).map(Number);
  // Date.UTC rolls invalid days over into the next month
  const date = new Date(Date.UTC(year, month - 1, day));
  if (
    date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1 ||
    date.getUTCDate() !== day
  ) {
    return undefined;
  }
  return {
    year,
    month,
    day,
    n: match[4] == null ? undefined : Number(match[4]),
  };
}

/** Orders by date, then `N`, with a missing `N` before any `N`. */
export function compareChronVer(a: ChronVer, b: ChronVer): number {
  return a.year - b.year || a.month - b.month || a.day - b.day ||
    (a.n ?? 0) - (b.n ?? 0);
}

/**
 * Whether the release `version` is newer than `other`. An `other` that isn't
 * ChronVer is a SemVer release from before Ende used ChronVer, so it's older.
 */
export function isNewerRelease(version: string, other: string): boolean {
  const parsed = parseChronVer(version);
  if (parsed == null) {
    throw new Error(`Not a ChronVer version: ${version}`);
  }
  const otherParsed = parseChronVer(other);
  return otherParsed == null || compareChronVer(parsed, otherParsed) > 0;
}

/**
 * The version for a release cut at `now`: today's UTC date, with `.N`
 * appended when a release (tag `v<version>`) already exists for today.
 */
export function nextReleaseVersion(
  now: Date,
  tags: { has(tag: string): boolean },
): string {
  const pad = (value: number) => value.toString().padStart(2, "0");
  const date = `${now.getUTCFullYear()}.${pad(now.getUTCMonth() + 1)}.${
    pad(now.getUTCDate())
  }`;
  let version = date;
  for (let n = 1; tags.has(`v${version}`); n++) {
    version = `${date}.${n}`;
  }
  return version;
}

/**
 * Why `tag` can't be released from a commit whose `cli/lib/version.txt` holds
 * `productVersion`, or `undefined` when it can. A release tag is `v` plus the
 * exact product version.
 */
export function releaseTagError(
  tag: string,
  productVersion: string,
): string | undefined {
  if (!tag.startsWith("v") || parseChronVer(tag) == null) {
    return `Tag ${tag} is not a ChronVer release tag (vYYYY.MM.DD[.N]).`;
  }
  if (tag !== `v${productVersion}`) {
    return `Tag ${tag} does not match cli/lib/version.txt (${productVersion}) ` +
      `at this commit; expected v${productVersion}.`;
  }
  return undefined;
}
