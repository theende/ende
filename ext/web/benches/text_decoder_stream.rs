// Copyright 2018-2026 the Deno authors. MIT license.

use std::sync::Arc;

use ende_bench_util::bench_js_sync;
use ende_bench_util::bench_or_profile;
use ende_bench_util::bencher::Bencher;
use ende_bench_util::bencher::benchmark_group;
use ende_core::Extension;
use ende_web::BlobStore;

fn setup() -> Vec<Extension> {
  // 08_text_encoding.js exposes `TextDecoder` via `core.loadExtScript`, so the
  // source has to be registered as lazy-loaded.
  ende_core::extension!(
    bench_setup,
    esm_entry_point = "ext:bench_setup/setup",
    esm = ["ext:bench_setup/setup" = {
      source = r#"
          const { TextDecoder } =
            __bootstrap.core.loadExtScript(
              "ext:ende_web/08_text_encoding.js",
            );

          // 8 KB of ASCII bytes, the common shape of an HTTP/JSON stream
          // chunk. Pure ASCII can never split a UTF-8 codepoint, so the
          // streaming decoder's internal state is irrelevant for these
          // chunks - the new fast path returns the V8 string directly.
          const asciiChunk = new Uint8Array(8192).fill(0x41);

          // A chunk that ends in the middle of a UTF-8 codepoint, forcing
          // the slow path even with the fast path in place.
          const mixedChunk = new Uint8Array(8192);
          for (let i = 0; i < 8190; i++) mixedChunk[i] = 0x41;
          mixedChunk[8190] = 0xC3; // start of U+00E9 - no continuation byte yet

          globalThis.decoder = new TextDecoder();
          globalThis.decodeAsciiStream = () =>
            globalThis.decoder.decode(asciiChunk, { stream: true });
          globalThis.decodeMixedStream = () =>
            globalThis.decoder.decode(mixedChunk, { stream: true });
        "#
    },],
  );

  vec![
    ende_webidl::ende_webidl::init(),
    ende_web::ende_web::init(
      Arc::new(BlobStore::default()),
      None,
      Default::default(),
      Default::default(),
    ),
    bench_setup::init(),
  ]
}

fn bench_decode_stream_ascii_8k(b: &mut Bencher) {
  bench_js_sync(b, r#"decodeAsciiStream();"#, setup);
}

fn bench_decode_stream_mixed_8k(b: &mut Bencher) {
  bench_js_sync(b, r#"decodeMixedStream();"#, setup);
}

benchmark_group!(
  benches,
  bench_decode_stream_ascii_8k,
  bench_decode_stream_mixed_8k,
);
bench_or_profile!(benches);
