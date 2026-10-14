// Copyright 2018-2026 the Deno authors. MIT license.

use std::io::BufReader;
use std::io::Cursor;
use std::path::PathBuf;

use base64::prelude::BASE64_STANDARD;
use base64::prelude::Engine;
use ende_runtime::UNSTABLE_ENV_VAR_NAMES;
use ende_runtime::colors;
use ende_runtime::ende_tls::deno_native_certs::load_native_certs;
use ende_runtime::ende_tls::rustls;
use ende_runtime::ende_tls::rustls::RootCertStore;
use ende_runtime::ende_tls::rustls_pemfile;
use ende_runtime::ende_tls::webpki_roots;
use serde::Deserialize;
use serde::Serialize;
use sys_traits::EnvVar;
use thiserror::Error;

pub fn has_trace_permissions_enabled(sys: &impl EnvVar) -> bool {
  has_flag_env_var(sys, "ENDE_TRACE_PERMISSIONS")
}

/// Whether the `ENDE_*` env var `name` (or its deprecated `DENO_*` name) is
/// set to `1`.
pub fn has_flag_env_var(sys: &impl EnvVar, name: &str) -> bool {
  match ende_env::env_var_os(sys, name) {
    Some(value) => value == "1",
    None => false,
  }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaData {
  /// The string is a file path
  File(String),
  /// The string holds the actual certificate
  Bytes(Vec<u8>),
}

impl CaData {
  pub fn parse(input: String) -> Option<Self> {
    if let Some(x) = input.strip_prefix("base64:") {
      Some(CaData::Bytes(BASE64_STANDARD.decode(x).ok()?))
    } else {
      Some(CaData::File(input))
    }
  }
}

#[derive(Error, Debug, Clone, deno_error::JsError)]
#[class(generic)]
pub enum RootCertStoreLoadError {
  #[error(
    "Unknown certificate store \"{0}\" specified (allowed: \"system,mozilla\")"
  )]
  UnknownStore(String),
  #[error("Unable to add pem file to certificate store: {0}")]
  FailedAddPemFile(String),
  #[error("Failed opening CA file: {0}")]
  CaFileOpenError(String),
  #[error("Failed to load platform certificates: {0}")]
  FailedNativeCerts(String),
}

/// Create and populate a root cert store based on the passed options and
/// environment.
pub fn get_root_cert_store(
  sys: &impl EnvVar,
  maybe_root_path: Option<PathBuf>,
  maybe_ca_stores: Option<Vec<String>>,
  maybe_ca_data: Option<CaData>,
) -> Result<RootCertStore, RootCertStoreLoadError> {
  let mut root_cert_store = RootCertStore::empty();
  let ca_stores: Vec<String> = maybe_ca_stores
    .or_else(|| {
      let env_ca_store = ende_env::env_var(sys, "ENDE_TLS_CA_STORE").ok()?;
      Some(
        env_ca_store
          .split(',')
          .map(|s| s.trim().to_string())
          .filter(|s| !s.is_empty())
          .collect(),
      )
    })
    .unwrap_or_else(|| vec!["mozilla".to_string()]);

  for store in ca_stores.iter() {
    match store.as_str() {
      "mozilla" => {
        root_cert_store.extend(webpki_roots::TLS_SERVER_ROOTS.to_vec());
      }
      "system" => {
        let roots = load_native_certs().map_err(|err| {
          RootCertStoreLoadError::FailedNativeCerts(err.to_string())
        })?;
        for root in roots {
          if let Err(err) = root_cert_store
            .add(rustls::pki_types::CertificateDer::from(root.0.clone()))
          {
            log::error!(
              "{}",
              colors::yellow(&format!(
                "Unable to add system certificate to certificate store: {:?}",
                err
              ))
            );
            let hex_encoded_root = faster_hex::hex_string(&root.0);
            log::error!("{}", colors::gray(&hex_encoded_root));
          }
        }
      }
      _ => {
        return Err(RootCertStoreLoadError::UnknownStore(store.clone()));
      }
    }
  }

  let ca_data = maybe_ca_data.or_else(|| {
    ende_env::env_var(sys, "ENDE_CERT")
      .ok()
      .and_then(CaData::parse)
  });
  if let Some(ca_data) = ca_data {
    let result = match ca_data {
      CaData::File(ca_file) => {
        let ca_file = if let Some(root) = &maybe_root_path {
          root.join(&ca_file)
        } else {
          PathBuf::from(ca_file)
        };
        let certfile = std::fs::File::open(ca_file).map_err(|err| {
          RootCertStoreLoadError::CaFileOpenError(err.to_string())
        })?;
        let mut reader = BufReader::new(certfile);
        rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()
      }
      CaData::Bytes(data) => {
        let mut reader = BufReader::new(Cursor::new(data));
        rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()
      }
    };

    match result {
      Ok(certs) => {
        root_cert_store.add_parsable_certificates(certs);
      }
      Err(e) => {
        return Err(RootCertStoreLoadError::FailedAddPemFile(e.to_string()));
      }
    }
  }

  Ok(root_cert_store)
}

#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnstableConfig {
  // TODO(bartlomieju): remove in Deno 2.5
  pub legacy_flag_enabled: bool, // --unstable
  pub lazy_dynamic_imports: bool,
  pub raw_imports: bool,
  pub sloppy_imports: bool,
  pub tsgo: bool,
  pub features: Vec<String>, // --unstabe-kv --unstable-cron
}

impl UnstableConfig {
  pub fn fill_with_env(&mut self, sys: &impl EnvVar) {
    fn maybe_set(sys: &impl EnvVar, value: &mut bool, var_name: &str) {
      if !*value && has_flag_env_var(sys, var_name) {
        *value = true;
      }
    }

    maybe_set(
      sys,
      &mut self.lazy_dynamic_imports,
      UNSTABLE_ENV_VAR_NAMES.lazy_dynamic_imports,
    );
    maybe_set(sys, &mut self.tsgo, UNSTABLE_ENV_VAR_NAMES.tsgo);
    maybe_set(
      sys,
      &mut self.raw_imports,
      UNSTABLE_ENV_VAR_NAMES.raw_imports,
    );
    maybe_set(
      sys,
      &mut self.sloppy_imports,
      UNSTABLE_ENV_VAR_NAMES.sloppy_imports,
    );
  }
}
