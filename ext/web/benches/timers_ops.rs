// Copyright 2018-2026 the Deno authors. MIT license.

use ende_bench_util::bench_js_async;
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
        import { setTimeout } from "ext:ende_web/02_timers.js";
        globalThis.setTimeout = setTimeout;
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

fn bench_set_timeout(b: &mut Bencher) {
  bench_js_async(b, r#"setTimeout(() => {}, 0);"#, setup);
}

benchmark_group!(benches, bench_set_timeout,);
bench_or_profile!(benches);
