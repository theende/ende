// Copyright 2018-2026 the Deno authors. MIT license.

use ende_bench_util::bench_js_sync;
use ende_bench_util::bench_or_profile;
use ende_bench_util::bencher::Bencher;
use ende_bench_util::bencher::benchmark_group;
use ende_core::Extension;

fn setup() -> Vec<Extension> {
  ende_core::extension!(
    ende_webidl_bench,
    esm_entry_point = "ext:ende_webidl_bench/setup.js",
    esm = ["ext:ende_webidl_bench/setup.js" = "benches/dict.js"]
  );

  vec![ende_webidl::ende_webidl::init(), ende_webidl_bench::init()]
}

fn converter_undefined(b: &mut Bencher) {
  bench_js_sync(b, r#"TextDecodeOptions(undefined);"#, setup);
}

fn handwritten_baseline_undefined(b: &mut Bencher) {
  bench_js_sync(b, r#"handwrittenConverter(undefined)"#, setup);
}

fn converter_object(b: &mut Bencher) {
  bench_js_sync(b, r#"TextDecodeOptions({});"#, setup);
}

fn handwritten_baseline_object(b: &mut Bencher) {
  bench_js_sync(b, r#"handwrittenConverter({})"#, setup);
}

benchmark_group!(
  benches,
  converter_undefined,
  handwritten_baseline_undefined,
  converter_object,
  handwritten_baseline_object,
);
bench_or_profile!(benches);
