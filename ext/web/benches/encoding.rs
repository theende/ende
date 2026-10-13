// Copyright 2018-2026 the Deno authors. MIT license.

use ende_bench_util::bench_js_sync;
use ende_bench_util::bench_or_profile;
use ende_bench_util::bencher::Bencher;
use ende_bench_util::bencher::benchmark_group;
use ende_core::Extension;

#[derive(Clone)]
struct Permissions;

fn setup() -> Vec<Extension> {
  ende_core::extension!(
    bench_setup,
    esm_entry_point = "ext:bench_setup/setup",
    esm = ["ext:bench_setup/setup" = {
      source = r#"
        import { TextDecoder, TextEncoder } from "ext:ende_web/08_text_encoding.js";
        globalThis.TextDecoder = TextDecoder;
        globalThis.TextEncoder = TextEncoder;
        globalThis.hello12k = Ende.core.encode("hello world\n".repeat(1e3));
        globalThis.hello120 = Ende.core.encode("hello world\n".repeat(10));
        // Non-ASCII (4-byte UTF-8 emoji, ~2.4kB): exercises the non-ASCII path.
        globalThis.utf8_2k = Ende.core.encode("\u{1F600} hello \u{4E2D}".repeat(64));
        globalThis.dec = new TextDecoder();
        globalThis.helloShort = "hello world\n";
        globalThis.encInto = new TextEncoder();
        globalThis.dest = new Uint8Array(64);
      "#
    }],
    state = |state| {
      state.put(Permissions {});
    },
  );

  vec![
    ende_webidl::ende_webidl::init(),
    ende_web::ende_web::init(
      std::sync::Arc::new(ende_web::BlobStore::default())
        as std::sync::Arc<dyn ende_web::BlobStoreTrait>,
      None,
      Default::default(),
      Default::default(),
    ),
    bench_setup::init(),
  ]
}

fn bench_encode_12kb(b: &mut Bencher) {
  bench_js_sync(b, r#"new TextDecoder().decode(hello12k);"#, setup);
}

fn bench_decode_12kb_reused(b: &mut Bencher) {
  bench_js_sync(b, r#"dec.decode(hello12k);"#, setup);
}

fn bench_decode_120b_fresh(b: &mut Bencher) {
  bench_js_sync(b, r#"new TextDecoder().decode(hello120);"#, setup);
}

fn bench_decode_120b_reused(b: &mut Bencher) {
  bench_js_sync(b, r#"dec.decode(hello120);"#, setup);
}

fn bench_decode_utf8_2k_reused(b: &mut Bencher) {
  bench_js_sync(b, r#"dec.decode(utf8_2k);"#, setup);
}

fn bench_encode_into_short(b: &mut Bencher) {
  bench_js_sync(b, r#"encInto.encodeInto(helloShort, dest);"#, setup);
}

benchmark_group!(
  benches,
  bench_encode_12kb,
  bench_decode_12kb_reused,
  bench_decode_120b_fresh,
  bench_decode_120b_reused,
  bench_decode_utf8_2k_reused,
  bench_encode_into_short,
);
bench_or_profile!(benches);
