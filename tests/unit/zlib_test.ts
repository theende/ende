// Copyright 2026 the Ende authors. MIT license.
import { assert, assertEquals, assertThrows } from "./test_util.ts";

const FORMATS = ["deflate", "deflate-raw", "gzip"] as const;

// Repetitive enough to compress, varied enough to need real Huffman codes.
function sampleData(size: number): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(size);
  let seed = 1;
  for (let i = 0; i < size; i++) {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    out[i] = i % 64 < 48 ? 97 + (i % 26) : seed & 0xff;
  }
  return out;
}

function concat(...parts: Uint8Array[]): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

async function readAll(stream: ReadableStream<Uint8Array>) {
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

Deno.test(function zlibRoundTripEachFormat() {
  const input = sampleData(100_000);
  for (const format of FORMATS) {
    const compressed = Ende.deflateSync(input, { format });
    assert(compressed.length < input.length, format);
    const result = new Ende.Inflater({ format }).push(compressed);
    assertEquals(result.consumed, compressed.length, format);
    assertEquals(result.done, true, format);
    assertEquals(result.data, input, format);
  }
});

Deno.test(function zlibDefaultFormatIsDeflate() {
  const input = sampleData(1000);
  const compressed = Ende.deflateSync(input);
  // zlib header: CM 8, 32K window
  assertEquals(compressed[0], 0x78);
  assertEquals(new Ende.Inflater().push(compressed).data, input);
  assertEquals(
    new Ende.Inflater({ format: "deflate" }).push(compressed).data,
    input,
  );
});

Deno.test(function zlibRoundTripEmpty() {
  for (const format of FORMATS) {
    const compressed = Ende.deflateSync(new Uint8Array(), { format });
    const result = new Ende.Inflater({ format }).push(compressed);
    assertEquals(result.consumed, compressed.length, format);
    assertEquals(result.done, true, format);
    assertEquals(result.data, new Uint8Array(), format);
  }
});

Deno.test(function zlibInflaterStopsAtEndOfStream() {
  const first = sampleData(5000);
  const second = sampleData(7000).reverse();
  const junk = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9]);
  for (const format of FORMATS) {
    const a = Ende.deflateSync(first, { format });
    const b = Ende.deflateSync(second, { format });
    const all = concat(a, b, junk);

    const r1 = new Ende.Inflater({ format }).push(all);
    assertEquals(r1.consumed, a.length, format);
    assertEquals(r1.done, true, format);
    assertEquals(r1.data, first, format);

    const r2 = new Ende.Inflater({ format }).push(all.subarray(r1.consumed));
    assertEquals(r2.consumed, b.length, format);
    assertEquals(r2.done, true, format);
    assertEquals(r2.data, second, format);
  }
});

Deno.test(function zlibInflaterByteByByte() {
  const input = sampleData(3000);
  const junk = new Uint8Array([0xde, 0xad, 0xbe, 0xef]);
  for (const format of FORMATS) {
    const compressed = Ende.deflateSync(input, { format });
    const all = concat(compressed, junk);
    const inflater = new Ende.Inflater({ format });
    const chunks: Uint8Array[] = [];
    let consumed = 0;
    for (let i = 0; i < all.length; i++) {
      const result = inflater.push(all.subarray(i, i + 1));
      consumed += result.consumed;
      chunks.push(result.data);
      if (i < compressed.length) {
        assertEquals(result.consumed, 1, `${format} byte ${i}`);
      } else {
        assertEquals(result.consumed, 0, `${format} byte ${i}`);
        assertEquals(result.data.length, 0, `${format} byte ${i}`);
      }
      assertEquals(
        result.done,
        i >= compressed.length - 1,
        `${format} byte ${i}`,
      );
    }
    assertEquals(consumed, compressed.length, format);
    assertEquals(concat(...chunks), input, format);
  }
});

Deno.test(function zlibInflaterIncompleteStream() {
  const input = sampleData(10_000);
  for (const format of FORMATS) {
    const compressed = Ende.deflateSync(input, { format });
    const inflater = new Ende.Inflater({ format });
    const head = inflater.push(compressed.subarray(0, compressed.length - 1));
    assertEquals(head.consumed, compressed.length - 1, format);
    assertEquals(head.done, false, format);
    const tail = inflater.push(compressed.subarray(compressed.length - 1));
    assertEquals(tail.consumed, 1, format);
    assertEquals(tail.done, true, format);
    assertEquals(concat(head.data, tail.data), input, format);
  }
});

Deno.test(function zlibInflaterPushAfterDone() {
  const compressed = Ende.deflateSync(sampleData(100));
  const inflater = new Ende.Inflater();
  assertEquals(inflater.push(compressed).done, true);
  for (
    const chunk of [compressed, new Uint8Array([1, 2, 3]), new Uint8Array()]
  ) {
    const result = inflater.push(chunk);
    assertEquals(result.consumed, 0);
    assertEquals(result.data, new Uint8Array());
    assertEquals(result.done, true);
  }
});

Deno.test(function zlibInflaterMaxOutputBytes() {
  const input = sampleData(50_000);
  for (const format of FORMATS) {
    const compressed = Ende.deflateSync(input, { format });
    const exact = new Ende.Inflater({ format, maxOutputBytes: input.length })
      .push(compressed);
    assertEquals(exact.done, true, format);
    assertEquals(exact.data, input, format);

    assertThrows(
      () =>
        new Ende.Inflater({ format, maxOutputBytes: input.length - 1 })
          .push(compressed),
      RangeError,
      "maxOutputBytes",
    );
  }
});

Deno.test(function zlibInflaterMaxOutputBytesAcrossPushes() {
  const input = sampleData(20_000);
  const compressed = Ende.deflateSync(input);
  const half = compressed.length >> 1;

  const exact = new Ende.Inflater({ maxOutputBytes: input.length });
  const a = exact.push(compressed.subarray(0, half));
  const b = exact.push(compressed.subarray(half));
  assertEquals(b.done, true);
  assertEquals(concat(a.data, b.data), input);

  const short = new Ende.Inflater({ maxOutputBytes: input.length - 1 });
  assertThrows(() => {
    short.push(compressed.subarray(0, half));
    short.push(compressed.subarray(half));
  }, RangeError);
});

Deno.test(function zlibInflaterMaxOutputBytesZero() {
  const compressed = Ende.deflateSync(new Uint8Array());
  const result = new Ende.Inflater({ maxOutputBytes: 0 }).push(compressed);
  assertEquals(result.done, true);
  assertEquals(result.data.length, 0);
  assertThrows(
    () =>
      new Ende.Inflater({ maxOutputBytes: 0 }).push(
        Ende.deflateSync(new Uint8Array([1])),
      ),
    RangeError,
  );
});

Deno.test(function zlibInflaterBombIsStopped() {
  // 64 MiB of zeros deflates to about 64 KiB.
  const compressed = Ende.deflateSync(new Uint8Array(64 * 1024 * 1024));
  assertThrows(
    () => new Ende.Inflater({ maxOutputBytes: 1024 }).push(compressed),
    RangeError,
    "maxOutputBytes",
  );
});

Deno.test(function zlibInflaterInvalidMaxOutputBytes() {
  for (const maxOutputBytes of [-1, 1.5, NaN, Infinity]) {
    assertThrows(
      () => new Ende.Inflater({ maxOutputBytes }),
      RangeError,
      undefined,
      String(maxOutputBytes),
    );
  }
});

Deno.test(function zlibInflaterCorruptInput() {
  const input = sampleData(5000);
  const cases: [Ende.ZlibFormat, Uint8Array<ArrayBuffer>][] = [
    // Bad zlib header check bits.
    ["deflate", new Uint8Array([0x78, 0x00, 1, 2, 3])],
    // Invalid block type 3.
    ["deflate-raw", new Uint8Array([0xff, 0xff, 0xff, 0xff])],
    // Bad gzip magic.
    ["gzip", new Uint8Array([0x1f, 0x8c, 8, 0, 0, 0, 0, 0, 0, 0xff])],
  ];
  // A flipped bit in the trailer: zlib's Adler-32 is the last 4 bytes, gzip's
  // CRC-32 and size are the last 8.
  for (
    const [format, fromEnd] of [
      ["deflate", 1],
      ["gzip", 5],
      ["gzip", 1],
    ] as const
  ) {
    const bad = Ende.deflateSync(input, { format });
    bad[bad.length - fromEnd] ^= 1;
    cases.push([format, bad]);
  }

  for (const [format, data] of cases) {
    assertThrows(
      () => new Ende.Inflater({ format }).push(data),
      TypeError,
      undefined,
      format,
    );
  }
});

Deno.test(function zlibInflaterThrowsAfterFailure() {
  const inflater = new Ende.Inflater();
  assertThrows(() => inflater.push(new Uint8Array([0x78, 0x00])), TypeError);
  assertThrows(() => inflater.push(Ende.deflateSync(new Uint8Array())));
});

Deno.test(function zlibGzipHeaderFields() {
  // FEXTRA, FNAME, FCOMMENT and FHCRC, followed by an empty deflate block.
  const header = [
    0x1f,
    0x8b,
    8,
    0x02 | 0x04 | 0x08 | 0x10,
    0,
    0,
    0,
    0,
    0,
    0xff,
    3,
    0,
    ...[0x41, 0x42, 0x43],
    ...[0x61, 0x2e, 0x74, 0x78, 0x74, 0],
    ...[0x68, 0x69, 0],
  ];
  // The header CRC is the low 16 bits of the CRC-32 of the header so far.
  let crc = 0xffffffff;
  for (const byte of header) {
    crc ^= byte;
    for (let k = 0; k < 8; k++) {
      crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
    }
  }
  crc = (crc ^ 0xffffffff) >>> 0;
  const body = Ende.deflateSync(new TextEncoder().encode("hello"), {
    format: "deflate-raw",
  });
  const trailer = Ende.deflateSync(new TextEncoder().encode("hello"), {
    format: "gzip",
  }).subarray(-8);
  const member = concat(
    new Uint8Array([...header, crc & 0xff, (crc >>> 8) & 0xff]),
    body,
    trailer,
  );
  const result = new Ende.Inflater({ format: "gzip" }).push(member);
  assertEquals(result.consumed, member.length);
  assertEquals(result.done, true);
  assertEquals(new TextDecoder().decode(result.data), "hello");

  const badHeaderCrc = member.slice();
  badHeaderCrc[header.length] ^= 1;
  assertThrows(
    () => new Ende.Inflater({ format: "gzip" }).push(badHeaderCrc),
    TypeError,
  );
});

Deno.test(function zlibDeflateLevel() {
  const input = sampleData(100_000);
  const stored = Ende.deflateSync(input, { level: 0 });
  const best = Ende.deflateSync(input, { level: 9 });
  assert(stored.length > input.length);
  assert(best.length < stored.length);
  assertEquals(new Ende.Inflater().push(stored).data, input);
  assertEquals(new Ende.Inflater().push(best).data, input);
  for (let level = 0; level <= 9; level++) {
    const compressed = Ende.deflateSync(input, { format: "gzip", level });
    assertEquals(
      new Ende.Inflater({ format: "gzip" }).push(compressed).data,
      input,
    );
  }
});

Deno.test(function zlibDeflateInvalidLevel() {
  for (const level of [-1, 10, 1.5, NaN]) {
    assertThrows(
      () => Ende.deflateSync(new Uint8Array(1), { level }),
      RangeError,
      undefined,
      String(level),
    );
  }
});

Deno.test(function zlibInvalidFormat() {
  assertThrows(
    // @ts-expect-error testing an invalid format
    () => Ende.deflateSync(new Uint8Array(1), { format: "brotli" }),
    TypeError,
  );
  assertThrows(
    // @ts-expect-error testing an invalid format
    () => new Ende.Inflater({ format: "zip" }),
    TypeError,
  );
});

Deno.test(function zlibInvalidChunk() {
  // @ts-expect-error testing an invalid chunk
  assertThrows(() => new Ende.Inflater().push("abc"), TypeError);
  // @ts-expect-error testing an invalid chunk
  assertThrows(() => Ende.deflateSync([1, 2, 3]), TypeError);
});

Deno.test(function zlibBufferSourceInputs() {
  const input = sampleData(4000);
  const compressed = Ende.deflateSync(input);
  // An offset view, so that the byte offset matters.
  const padded = new Uint8Array(compressed.length + 3);
  padded.set(compressed, 3);

  const fromBuffer = new Ende.Inflater().push(compressed.slice().buffer);
  assertEquals(fromBuffer.data, input);
  const fromView = new Ende.Inflater().push(
    new DataView(padded.buffer, 3, compressed.length),
  );
  assertEquals(fromView.consumed, compressed.length);
  assertEquals(fromView.data, input);

  assertEquals(Ende.deflateSync(input.slice().buffer), compressed);
  assertEquals(
    Ende.deflateSync(new DataView(input.buffer, 0, input.length)),
    compressed,
  );
  const words = new Uint32Array(input.slice().buffer);
  assertEquals(Ende.deflateSync(words), compressed);
});

Deno.test(async function zlibInteropWithCompressionStreams() {
  const input = sampleData(60_000);
  for (const format of FORMATS) {
    const ours = Ende.deflateSync(input, { format });
    const decoded = await readAll(
      new Blob([ours]).stream().pipeThrough(new DecompressionStream(format)),
    );
    assertEquals(decoded, input, format);

    const theirs = await readAll(
      new Blob([input]).stream().pipeThrough(new CompressionStream(format)),
    );
    const result = new Ende.Inflater({ format }).push(theirs);
    assertEquals(result.consumed, theirs.length, format);
    assertEquals(result.done, true, format);
    assertEquals(result.data, input, format);
  }
});
