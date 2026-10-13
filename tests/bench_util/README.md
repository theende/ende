# Benching utility for `ende_core` op system

Example:

```rust
use ende_bench_util::bench_js_sync;
use ende_bench_util::bench_or_profile;
use ende_bench_util::bencher::Bencher;
use ende_bench_util::bencher::benchmark_group;
use ende_core::Extension;

#[op2]
#[number]
fn op_nop() -> usize {
  9
}

fn setup() -> Vec<Extension> {
  vec![Extension {
    name: "my_ext",
    ops: std::borrow::Cow::Borrowed(&[op_nop::DECL]),
  }]
}

fn bench_op_nop(b: &mut Bencher) {
  bench_js_sync(b, r#"Ende.core.ops.op_nop();"#, setup);
}

benchmark_group!(benches, bench_op_nop);
bench_or_profile!(benches);
```
