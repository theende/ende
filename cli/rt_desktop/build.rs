// Copyright 2018-2026 the Deno authors. MIT license.

#![allow(clippy::disallowed_methods, reason = "build code")]

fn main() {
  // Skip building from docs.rs.
  if std::env::var_os("DOCS_RS").is_some() {
    return;
  }

  print_cdylib_linker_flags();
}

fn print_cdylib_linker_flags() {
  #[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
  ))]
  {
    // V8 (via rusty_v8's `use_custom_libcxx`) statically links its own
    // libc++/libc++abi into this cdylib. A desktop backend (laufey) `dlopen`s
    // us into a process that already uses the system libstdc++. Without hiding
    // our bundled C++ runtime symbols, ELF interposition binds our internal
    // `__cxa_guard_acquire` (and friends) to the host's libstdc++, whose guard
    // layout differs — the runtime aborts at static init with
    // "libc++abi: __cxa_guard_acquire failed to acquire mutex" before
    // `Deno.serve` ever runs (denoland/deno#35381).
    //
    // `--exclude-libs,ALL` localizes every static-archive symbol (including the
    // bundled libc++abi) so our calls resolve to our own copy. The
    // `laufey_runtime_*` entry points live in this crate's own objects (not an
    // archive), so they stay visible.
    println!("cargo:rustc-cdylib-link-arg=-Wl,--exclude-libs,ALL");
  }
}
