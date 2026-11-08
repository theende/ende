#!/usr/bin/env -S deno run --allow-net=registry.npmjs.org --allow-read --allow-write --allow-run=zip
// Copyright 2026 the Ende authors. MIT license.
// deno-lint-ignore-file no-console

// Lays out the pinned TypeScript 7 compiler for upload to dl.ende.sh.
//
// For each target in cli/tsc7_sums.lock it downloads the npm tarball from
// registry.npmjs.org, checks it against the pinned sha512 integrity, extracts
// `tsc` (`tsc.exe` on Windows), the `lib.*.d.ts` files it loads from its own
// directory at runtime, and the package's LICENSE and NOTICE.txt, and checks
// the SHA-256 of `tsc`. It writes:
//
//   <out-dir>/tsc/<version>/<target>/{tsc[.exe],lib.*.d.ts,LICENSE,NOTICE.txt}
//   <out-dir>/tsc/<version>/tsc-<target>.zip          the same files
//   <out-dir>/tsc/<version>/tsc-<target>.zip.sha256sum
//   <out-dir>/tsc/<version>/SHA256SUMS                every file above
//
// It uploads nothing; it prints the upload command.
//
// Usage: tools/tsc7_mirror.ts <out-dir>

const LOCK_URL = new URL("../cli/tsc7_sums.lock", import.meta.url);

interface Pin {
  target: string;
  pkg: string;
  integrity: string;
  tscSha256: string;
}

function parseLock(text: string): { version: string; pins: Pin[] } {
  let version: string | undefined;
  const pins: Pin[] = [];
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    const directive = line.match(/^#\s*version:\s*(\S+)$/);
    if (directive) {
      version = directive[1];
      continue;
    }
    if (line === "" || line.startsWith("#")) continue;
    const fields = line.split(/\s+/);
    if (fields.length !== 4) {
      throw new Error(`cli/tsc7_sums.lock: malformed line: ${line}`);
    }
    const [target, pkg, integrity, tscSha256] = fields;
    pins.push({ target, pkg, integrity, tscSha256 });
  }
  if (!version || !/^\d+\.\d+\.\d+$/.test(version)) {
    throw new Error("cli/tsc7_sums.lock: missing `# version: X.Y.Z`");
  }
  return { version, pins };
}

function tarballUrl(pkg: string, version: string): string {
  const name = pkg.split("/").pop()!;
  return `https://registry.npmjs.org/${pkg}/-/${name}-${version}.tgz`;
}

async function digest(
  algorithm: string,
  data: Uint8Array<ArrayBuffer>,
): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest(algorithm, data));
}

function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function base64(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes));
}

async function gunzip(
  data: Uint8Array<ArrayBuffer>,
): Promise<Uint8Array<ArrayBuffer>> {
  const stream = new Blob([data]).stream().pipeThrough(
    new DecompressionStream("gzip"),
  );
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/** Regular files of a (ustar or pax) tar archive, by path. */
function untar(
  data: Uint8Array<ArrayBuffer>,
): Map<string, Uint8Array<ArrayBuffer>> {
  const decoder = new TextDecoder();
  const field = (block: Uint8Array, start: number, len: number) =>
    decoder.decode(block.subarray(start, start + len)).replace(/\0.*$/s, "");
  const files = new Map<string, Uint8Array<ArrayBuffer>>();
  let paxPath: string | undefined;
  let offset = 0;
  while (offset + 512 <= data.length) {
    const header = data.subarray(offset, offset + 512);
    if (header.every((b) => b === 0)) break;
    const size = parseInt(field(header, 124, 12).trim() || "0", 8);
    const type = field(header, 156, 1) || "0";
    const prefix = field(header, 345, 155);
    let path = prefix
      ? `${prefix}/${field(header, 0, 100)}`
      : field(header, 0, 100);
    const body = data.subarray(offset + 512, offset + 512 + size);
    offset += 512 + Math.ceil(size / 512) * 512;
    if (type === "x") {
      // pax extended header: `<len> <key>=<value>\n` records.
      const match = decoder.decode(body).match(/^\d+ path=(.*)$/m);
      paxPath = match?.[1];
      continue;
    }
    if (paxPath !== undefined) {
      path = paxPath;
      paxPath = undefined;
    }
    if (type === "0") files.set(path, body);
  }
  return files;
}

function formatSize(bytes: number): string {
  return `${(bytes / 1e6).toFixed(2)} MB`;
}

async function sha256sumLine(path: string, name: string): Promise<string> {
  return `${
    hex(await digest("SHA-256", await Deno.readFile(path)))
  }  ${name}\n`;
}

async function mirrorTarget(pin: Pin, version: string, versionDir: string) {
  const url = tarballUrl(pin.pkg, version);
  console.error(`Downloading ${url}`);
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`${url}: HTTP ${response.status}`);
  }
  const tarball = new Uint8Array(await response.arrayBuffer());
  const integrity = `sha512-${base64(await digest("SHA-512", tarball))}`;
  if (integrity !== pin.integrity) {
    throw new Error(
      `${url}: integrity ${integrity} doesn't match the pinned ${pin.integrity}`,
    );
  }

  const exe = pin.target.includes("windows") ? "tsc.exe" : "tsc";
  const files = untar(await gunzip(tarball));
  const wanted = new Map<string, string>(); // archive path -> output name
  for (const path of files.keys()) {
    const name = path.slice("package/lib/".length);
    if (
      path.startsWith("package/lib/") &&
      (name === exe || /^lib(\.[a-z0-9]+)*\.d\.ts$/.test(name))
    ) {
      wanted.set(path, name);
    }
  }
  wanted.set("package/LICENSE", "LICENSE");
  wanted.set("package/NOTICE.txt", "NOTICE.txt");

  const tsc = files.get(`package/lib/${exe}`);
  if (!tsc) throw new Error(`${url}: no package/lib/${exe}`);
  const tscSha256 = hex(await digest("SHA-256", tsc));
  if (tscSha256 !== pin.tscSha256) {
    throw new Error(
      `${url}: ${exe} SHA-256 ${tscSha256} doesn't match the pinned ${pin.tscSha256}`,
    );
  }

  const targetDir = `${versionDir}/${pin.target}`;
  await Deno.mkdir(targetDir);
  const names: string[] = [];
  for (const [path, name] of wanted) {
    const contents = files.get(path);
    if (!contents) throw new Error(`${url}: no ${path}`);
    await Deno.writeFile(`${targetDir}/${name}`, contents, {
      mode: name === exe ? 0o755 : 0o644,
    });
    names.push(name);
  }
  names.sort();

  const zipName = `tsc-${pin.target}.zip`;
  const zip = await new Deno.Command("zip", {
    args: ["-q", "-9", "-X", `../${zipName}`, ...names],
    cwd: targetDir,
  }).output();
  if (!zip.success) {
    throw new Error(`zip ${zipName}: ${new TextDecoder().decode(zip.stderr)}`);
  }
  await Deno.writeTextFile(
    `${versionDir}/${zipName}.sha256sum`,
    await sha256sumLine(`${versionDir}/${zipName}`, zipName),
  );

  const libCount = names.filter((n) => n.endsWith(".d.ts")).length;
  const zipSize = (await Deno.stat(`${versionDir}/${zipName}`)).size;
  console.log(
    `${pin.target}: tarball ${formatSize(tarball.length)}, ${exe} ${
      formatSize(tsc.length)
    }, ${libCount} lib files, ${zipName} ${formatSize(zipSize)}`,
  );
}

async function listFiles(dir: string, prefix = ""): Promise<string[]> {
  const out: string[] = [];
  for await (const entry of Deno.readDir(dir)) {
    const rel = prefix + entry.name;
    if (entry.isDirectory) {
      out.push(...await listFiles(`${dir}/${entry.name}`, `${rel}/`));
    } else {
      out.push(rel);
    }
  }
  return out.sort();
}

async function main() {
  const outDir = Deno.args[0];
  if (Deno.args.length !== 1 || !outDir) {
    console.error("Usage: tools/tsc7_mirror.ts <out-dir>");
    Deno.exit(1);
  }
  const { version, pins } = parseLock(await Deno.readTextFile(LOCK_URL));
  const versionDir = `${outDir}/tsc/${version}`;
  try {
    await Deno.stat(versionDir);
    throw new Error(`${versionDir} already exists; remove it first`);
  } catch (err) {
    if (!(err instanceof Deno.errors.NotFound)) throw err;
  }
  await Deno.mkdir(versionDir, { recursive: true });

  for (const pin of pins) {
    await mirrorTarget(pin, version, versionDir);
  }

  const files = await listFiles(versionDir);
  let sums = "";
  for (const file of files) {
    sums += await sha256sumLine(`${versionDir}/${file}`, file);
  }
  await Deno.writeTextFile(`${versionDir}/SHA256SUMS`, sums);

  console.log(`\nWrote ${files.length + 1} files under ${versionDir}:`);
  for (const file of [...files, "SHA256SUMS"]) {
    if (!file.includes("/") || !file.endsWith(".d.ts")) {
      console.log(`  ${file}`);
    }
  }
  console.log("  (plus the lib.*.d.ts files in each target directory)");
  console.log(
    `\nNothing was uploaded. To upload, with the dl.ende.sh R2 credentials the
release workflows use (DL_S3_ACCESS_KEY_ID, DL_S3_SECRET_ACCESS_KEY,
DL_S3_ENDPOINT, DL_S3_BUCKET):

  AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... \\
  AWS_ENDPOINT_URL_S3=... AWS_DEFAULT_REGION=auto \\
  AWS_REQUEST_CHECKSUM_CALCULATION=when_required \\
  AWS_RESPONSE_CHECKSUM_VALIDATION=when_required \\
  aws s3 cp "${versionDir}/" "s3://$DL_S3_BUCKET/tsc/${version}/" --recursive`,
  );
}

await main();
