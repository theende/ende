// Copyright 2018-2026 the Deno authors. MIT license.

use std::path::PathBuf;

use ende_core::url::Url;

use crate::http_util::HttpClientProvider;

pub struct BinNameResolver<'a> {
  http_client_provider: &'a HttpClientProvider,
}

impl<'a> BinNameResolver<'a> {
  pub fn new(http_client_provider: &'a HttpClientProvider) -> Self {
    Self {
      http_client_provider,
    }
  }

  pub async fn infer_name_from_url(&self, url: &Url) -> Option<String> {
    // If there's an absolute url with no path, eg. https://my-cli.com
    // perform a request, and see if it redirects another file instead.
    let mut url = url.clone();

    if matches!(url.scheme(), "http" | "https")
      && url.path() == "/"
      && let Ok(client) = self.http_client_provider.get_or_create()
      && let Ok(redirected_url) = client
        .get_redirected_url(url.clone(), &Default::default())
        .await
    {
      url = redirected_url;
    }

    let percent_decode =
      percent_encoding::percent_decode(url.path().as_bytes());
    #[cfg(unix)]
    let path = {
      use std::os::unix::prelude::OsStringExt;
      PathBuf::from(std::ffi::OsString::from_vec(
        percent_decode.collect::<Vec<u8>>(),
      ))
    };
    #[cfg(windows)]
    let path = PathBuf::from(percent_decode.decode_utf8_lossy().as_ref());

    let mut stem = path.file_stem()?.to_string_lossy();
    if matches!(stem.as_ref(), "main" | "mod" | "index" | "cli")
      && let Some(parent_name) = path.parent().and_then(|p| p.file_name())
    {
      stem = parent_name.to_string_lossy();
    }

    // if atmark symbol appears in the index other than 0 (e.g. `foo@bar`) we use
    // the former part as the inferred name because the latter part is most likely
    // a version number.
    match stem.find('@') {
      Some(at_index) if at_index > 0 => {
        stem = stem.split_at(at_index).0.to_string().into();
      }
      _ => {}
    }

    Some(stem.to_string())
  }
}

#[cfg(test)]
mod test {
  use ende_core::url::Url;

  use super::BinNameResolver;
  use crate::http_util::HttpClientProvider;

  async fn infer_name_from_url(url: &Url) -> Option<String> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let http_client = HttpClientProvider::new(None, None);
    let resolver = BinNameResolver::new(&http_client);
    resolver.infer_name_from_url(url).await
  }

  #[tokio::test]
  async fn install_infer_name_from_url() {
    assert_eq!(
      infer_name_from_url(
        &Url::parse("https://example.com/abc/server.ts").unwrap()
      )
      .await,
      Some("server".to_string())
    );
    assert_eq!(
      infer_name_from_url(
        &Url::parse("https://example.com/abc/main.ts").unwrap()
      )
      .await,
      Some("abc".to_string())
    );
    assert_eq!(
      infer_name_from_url(
        &Url::parse("https://example.com/abc/mod.ts").unwrap()
      )
      .await,
      Some("abc".to_string())
    );
    assert_eq!(
      infer_name_from_url(
        &Url::parse("https://example.com/ab%20c/mod.ts").unwrap()
      )
      .await,
      Some("ab c".to_string())
    );
    assert_eq!(
      infer_name_from_url(
        &Url::parse("https://example.com/abc/index.ts").unwrap()
      )
      .await,
      Some("abc".to_string())
    );
    assert_eq!(
      infer_name_from_url(
        &Url::parse("https://example.com/abc/cli.ts").unwrap()
      )
      .await,
      Some("abc".to_string())
    );
    assert_eq!(
      infer_name_from_url(&Url::parse("https://example.com/main.ts").unwrap())
        .await,
      Some("main".to_string())
    );
    assert_eq!(
      infer_name_from_url(&Url::parse("https://example.com").unwrap()).await,
      None
    );
    assert_eq!(
      infer_name_from_url(&Url::parse("file:///abc/server.ts").unwrap()).await,
      Some("server".to_string())
    );
    assert_eq!(
      infer_name_from_url(&Url::parse("file:///abc/main.ts").unwrap()).await,
      Some("abc".to_string())
    );
    assert_eq!(
      infer_name_from_url(&Url::parse("file:///ab%20c/main.ts").unwrap()).await,
      Some("ab c".to_string())
    );
    assert_eq!(
      infer_name_from_url(&Url::parse("file:///main.ts").unwrap()).await,
      Some("main".to_string())
    );
    assert_eq!(
      infer_name_from_url(&Url::parse("file:///").unwrap()).await,
      None
    );
    assert_eq!(
      infer_name_from_url(
        &Url::parse("https://example.com/abc@0.1.0").unwrap()
      )
      .await,
      Some("abc".to_string())
    );
    assert_eq!(
      infer_name_from_url(
        &Url::parse("https://example.com/abc@0.1.0/main.ts").unwrap()
      )
      .await,
      Some("abc".to_string())
    );
    assert_eq!(
      infer_name_from_url(
        &Url::parse("https://example.com/abc@def@ghi").unwrap()
      )
      .await,
      Some("abc".to_string())
    );
    assert_eq!(
      infer_name_from_url(&Url::parse("https://example.com/@abc.ts").unwrap())
        .await,
      Some("@abc".to_string())
    );
    assert_eq!(
      infer_name_from_url(
        &Url::parse("https://example.com/@abc/mod.ts").unwrap()
      )
      .await,
      Some("@abc".to_string())
    );
    assert_eq!(
      infer_name_from_url(&Url::parse("file:///@abc.ts").unwrap()).await,
      Some("@abc".to_string())
    );
    assert_eq!(
      infer_name_from_url(&Url::parse("file:///@abc/cli.ts").unwrap()).await,
      Some("@abc".to_string())
    );
    assert_eq!(
      infer_name_from_url(&Url::parse("jsr:@std/http@1/file-server").unwrap())
        .await,
      Some("file-server".to_string())
    );
  }
}
