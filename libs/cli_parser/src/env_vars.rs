// Copyright 2018-2026 the Deno authors. MIT license.
//! Environment variables documented in `ende --help`.

#[derive(serde::Serialize)]
pub struct EnvVar {
  pub name: &'static str,
  pub description: &'static str,
  pub example: Option<&'static str>,
}

pub static ENV_VARS: &[EnvVar] = &[
  EnvVar {
    name: "ENDE_AUTH_TOKENS",
    description: "A semi-colon separated list of bearer tokens and hostnames\nto use when fetching remote modules from private repositories",
    example: Some(r#"(e.g. "abcde12345@deno.land;54321edcba@github.com")"#),
  },
  EnvVar {
    name: "ENDE_CACHE_DB_MODE",
    description: "Controls whether Web cache should use disk based or in-memory database.",
    example: None,
  },
  EnvVar {
    name: "ENDE_CERT",
    description: "Load certificate authorities from PEM encoded file.",
    example: None,
  },
  EnvVar {
    name: "ENDE_COVERAGE_DIR",
    description: "Set the directory for collecting code coverage profiles.\nEquivalent to using the --coverage flag.",
    example: None,
  },
  EnvVar {
    name: "ENDE_DIR",
    description: "Set the cache directory",
    example: None,
  },
  EnvVar {
    name: "ENDE_INSTALL_ROOT",
    description: "Set ende install's output directory",
    example: Some("(defaults to $HOME/.ende/bin)"),
  },
  EnvVar {
    name: "ENDE_JOBS",
    description: "Number of parallel workers used for the --parallel flag with the test\nsubcommand. Defaults to the number of available CPUs.",
    example: None,
  },
  EnvVar {
    name: "ENDE_KV_DB_MODE",
    description: "Controls whether Ende.openKv() API should use disk based or in-memory\ndatabase.",
    example: None,
  },
  EnvVar {
    name: "ENDE_KV_DEFAULT_PATH",
    description: "Set the default path for Ende.openKv() when no path is provided.",
    example: None,
  },
  EnvVar {
    name: "ENDE_KV_PATH_PREFIX",
    description: "Set a prefix to be added to all Ende.openKv() paths.",
    example: None,
  },
  EnvVar {
    name: "ENDE_KV_REQUIRES_DISTRIBUTED_DATABASE",
    description: "Require Ende.openKv() to resolve to a distributed (remote) database.\nWhen set to \"error\", Ende.openKv() is always exposed and rejects with a\nclear message unless the resolved path is remote. When set to \"warn\", a\nlocal/in-memory fallback is allowed but logs a warning.",
    example: None,
  },
  EnvVar {
    name: "ENDE_EMIT_CACHE_MODE",
    description: "Control if the transpiled sources should be cached.",
    example: None,
  },
  EnvVar {
    name: "ENDE_NO_PROMPT",
    description: "Set to disable permission prompts on access\n(alternative to passing --no-prompt on invocation).",
    example: None,
  },
  EnvVar {
    name: "ENDE_NO_UPDATE_CHECK",
    description: "Set to disable checking if a newer Ende version is available",
    example: None,
  },
  EnvVar {
    name: "ENDE_PATCH_REACT_CVE",
    description: "Enable load-time source patches mitigating known React Server\nComponents CVEs (CVE-2025-55182, CVE-2025-55184).",
    example: None,
  },
  EnvVar {
    name: "ENDE_SERVE_ADDRESS",
    description: "Override address for Ende.serve",
    example: Some(
      r#"("tcp:0.0.0.0:8080", "unix:/tmp/ende.sock", or "vsock:1234:5678")"#,
    ),
  },
  EnvVar {
    name: "ENDE_SERVE_AUTOMATIC_COMPRESSION",
    description: "Set to 1 or true to enable automatic response body compression in Ende.serve by default.",
    example: None,
  },
  EnvVar {
    name: "ENDE_AUTO_SERVE",
    description: "If the entrypoint contains export default { fetch }, `ende run`\nbehaves like `ende serve`.",
    example: None,
  },
  EnvVar {
    name: "ENDE_TLS_CA_STORE",
    // Descriptions are plain text here (this table is shared by the plain
    // hand-written help renderer and the JSON reference). The original in
    // clap's `flags.rs` wrapped the `(defaults to "mozilla")` clause in
    // `cstr!` for dim coloring; that was intentionally dropped in the move —
    // it only affected the legacy clap `--help`, which is being removed.
    description: "Comma-separated list of order dependent certificate stores.\nPossible values: \"system\", \"mozilla\" (defaults to \"mozilla\")",
    example: None,
  },
  EnvVar {
    name: "ENDE_TRACE_PERMISSIONS",
    description: "Environmental variable to enable stack traces in permission prompts.",
    example: None,
  },
  EnvVar {
    name: "ENDE_TSC_BIN",
    description: "Path to a prebuilt native TypeScript compiler (tsc/tsgo) for `ende check` to use instead of downloading the pinned one.",
    example: None,
  },
  EnvVar {
    name: "ENDE_USE_CGROUPS",
    description: "Use cgroups to determine V8 memory limit.",
    example: None,
  },
  EnvVar {
    name: "ENDE_V8_FLAGS",
    description: "Set V8 command line options. Equivalent to using the --v8-flags flag;\nflags passed via --v8-flags are appended after these.",
    example: None,
  },
  EnvVar {
    name: "FORCE_COLOR",
    description: "Set force color output even if stdout isn't a tty.",
    example: None,
  },
  EnvVar {
    name: "HTTP_PROXY",
    description: "Proxy address for HTTP requests.",
    example: Some("(module downloads, fetch)"),
  },
  EnvVar {
    name: "HTTPS_PROXY",
    description: "Proxy address for HTTPS requests.",
    example: Some("(module downloads, fetch)"),
  },
  EnvVar {
    name: "NO_COLOR",
    description: "Set to disable color.",
    example: None,
  },
  EnvVar {
    name: "NO_PROXY",
    description: "Comma-separated list of hosts which do not use a proxy.",
    example: Some("(module downloads, fetch)"),
  },
  EnvVar {
    name: "SSLKEYLOGFILE",
    description: "Write TLS session keys to the specified file in NSS Key Log format\nfor debugging encrypted traffic with tools like Wireshark.",
    example: None,
  },
  EnvVar {
    name: "ENDE_USR2_MEMORY_TRIM",
    description: "If specified, listen for SIGUSR2 signal to try and free memory (Linux only).",
    example: None,
  },
];
