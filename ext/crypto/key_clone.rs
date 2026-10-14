// Copyright 2018-2026 the Deno authors. MIT license.

//! Structured-clone support for `CryptoKey`: rebuilds a key from the
//! snapshot produced by the host-object brand callback.

use deno_error::JsErrorBox;
use ende_core::v8;

use crate::CryptoError;
use crate::make_key::AlgorithmDict;
use crate::make_key::make_crypto_key;
use crate::shared::RawKeyData;

fn type_error(msg: String) -> CryptoError {
  CryptoError::Other(JsErrorBox::type_error(msg))
}

/// Body of `CryptoKey.fromCloneData(data)` — invoked by the JS
/// `registerCloneableResource("CryptoKey", ...)` callback to resurrect a
/// `CryptoKey` from the snapshot produced by the host-object brand
/// callback in [`crate::make_key`]. The snapshot has shape
/// `{ type: "CryptoKey", keyType, extractable, usages, algorithm, keyData }`.
pub fn from_clone_data<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  data: v8::Local<'s, v8::Value>,
) -> Result<v8::Local<'s, v8::Object>, CryptoError> {
  let obj = v8::Local::<v8::Object>::try_from(data).map_err(|_| {
    CryptoError::Other(JsErrorBox::type_error("Clone data must be an object"))
  })?;
  let key_type_str = read_string_member(scope, obj, b"keyType")
    .ok_or_else(|| type_error("Missing keyType".to_string()))?;
  let key_type = match key_type_str.as_str() {
    "public" => crate::crypto_key::CryptoKeyType::Public,
    "private" => crate::crypto_key::CryptoKeyType::Private,
    "secret" => crate::crypto_key::CryptoKeyType::Secret,
    _ => return Err(type_error("Invalid keyType".to_string())),
  };
  let extractable =
    read_bool_member(scope, obj, b"extractable").unwrap_or(true);
  let usages_strs =
    read_string_array(scope, obj, b"usages").unwrap_or_default();
  let alg_name = read_algorithm_name(scope, obj)
    .ok_or_else(|| type_error("Missing algorithm.name".to_string()))?;
  let alg = build_algorithm_dict_from_v8(scope, obj, &alg_name);
  let raw = read_key_data_from_v8(scope, obj)?;
  let usages: Vec<&str> = usages_strs.iter().map(String::as_str).collect();
  Ok(make_crypto_key(
    scope,
    key_type,
    extractable,
    &usages,
    alg,
    raw,
  ))
}

fn read_string_member<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  obj: v8::Local<'s, v8::Object>,
  field: &[u8],
) -> Option<String> {
  let key = v8::String::new_from_one_byte(
    scope,
    field,
    v8::NewStringType::Internalized,
  )?;
  let v = obj.get(scope, key.into())?;
  if v.is_undefined() || v.is_null() {
    return None;
  }
  Some(v.to_rust_string_lossy(scope))
}

fn read_bool_member<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  obj: v8::Local<'s, v8::Object>,
  field: &[u8],
) -> Option<bool> {
  let key = v8::String::new_from_one_byte(
    scope,
    field,
    v8::NewStringType::Internalized,
  )?;
  let v = obj.get(scope, key.into())?;
  if v.is_undefined() || v.is_null() {
    return None;
  }
  Some(v.boolean_value(scope))
}

fn read_string_array<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  obj: v8::Local<'s, v8::Object>,
  field: &[u8],
) -> Option<Vec<String>> {
  let key = v8::String::new_from_one_byte(
    scope,
    field,
    v8::NewStringType::Internalized,
  )?;
  let v = obj.get(scope, key.into())?;
  let arr = v8::Local::<v8::Array>::try_from(v).ok()?;
  let len = arr.length();
  let mut out = Vec::with_capacity(len as usize);
  for i in 0..len {
    let item = arr.get_index(scope, i)?;
    let s = item.to_string(scope)?;
    out.push(s.to_rust_string_lossy(scope));
  }
  Some(out)
}

fn read_algorithm_name<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  obj: v8::Local<'s, v8::Object>,
) -> Option<String> {
  let k = v8::String::new_from_one_byte(
    scope,
    b"algorithm",
    v8::NewStringType::Internalized,
  )?;
  let v = obj.get(scope, k.into())?;
  let alg = v8::Local::<v8::Object>::try_from(v).ok()?;
  read_string_member(scope, alg, b"name")
}

fn build_algorithm_dict_from_v8<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  obj: v8::Local<'s, v8::Object>,
  name: &str,
) -> AlgorithmDict {
  let mut dict = AlgorithmDict::new(name);
  let alg_key = v8::String::new(scope, "algorithm").unwrap();
  if let Some(alg_val) = obj.get(scope, alg_key.into())
    && let Ok(alg_obj) = v8::Local::<v8::Object>::try_from(alg_val)
  {
    let length_key = v8::String::new(scope, "length").unwrap();
    if let Some(l) = alg_obj.get(scope, length_key.into()).and_then(|v| {
      if v.is_undefined() {
        None
      } else {
        v.uint32_value(scope)
      }
    }) {
      dict.length = Some(l);
    }
    let curve_key = v8::String::new(scope, "namedCurve").unwrap();
    if let Some(s) = alg_obj.get(scope, curve_key.into()).and_then(|v| {
      if v.is_undefined() {
        None
      } else {
        Some(v.to_rust_string_lossy(scope))
      }
    }) {
      dict.named_curve = Some(s);
    }
    let hash_key = v8::String::new(scope, "hash").unwrap();
    if let Some(h) = alg_obj.get(scope, hash_key.into())
      && !h.is_undefined()
      && !h.is_null()
    {
      let h_name = if h.is_string() {
        Some(h.to_rust_string_lossy(scope))
      } else {
        v8::Local::<v8::Object>::try_from(h).ok().and_then(|ho| {
          let nk = v8::String::new(scope, "name").unwrap();
          ho.get(scope, nk.into())
            .map(|nv| nv.to_rust_string_lossy(scope))
        })
      };
      dict.hash_name = h_name;
    }
    let ml_key = v8::String::new(scope, "modulusLength").unwrap();
    if let Some(ml) = alg_obj.get(scope, ml_key.into()).and_then(|v| {
      if v.is_undefined() {
        None
      } else {
        v.uint32_value(scope)
      }
    }) {
      dict.modulus_length = Some(ml);
    }
    let pe_key = v8::String::new(scope, "publicExponent").unwrap();
    if let Some(pe) = alg_obj.get(scope, pe_key.into())
      && let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(pe)
    {
      let mut out = vec![0u8; view.byte_length()];
      let n = view.copy_contents(&mut out);
      out.truncate(n);
      dict.public_exponent = Some(out);
    }
  }
  dict
}

fn read_key_data_from_v8<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  obj: v8::Local<'s, v8::Object>,
) -> Result<RawKeyData, CryptoError> {
  let k = v8::String::new(scope, "keyData").unwrap();
  let v = obj
    .get(scope, k.into())
    .ok_or_else(|| type_error("Missing keyData".to_string()))?;
  // Match the shape produced by `key_data_to_jsval` in `make_key.rs`:
  // either `{ type, data }`, `{ seed, privateKey }`, or a bare
  // `Uint8Array` for `Raw`.
  if let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(v) {
    let mut out = vec![0u8; view.byte_length()];
    let n = view.copy_contents(&mut out);
    out.truncate(n);
    return Ok(RawKeyData::Raw(out.into_boxed_slice()));
  }
  let data_obj = v8::Local::<v8::Object>::try_from(v)
    .map_err(|_| type_error("keyData must be object/Uint8Array".to_string()))?;
  if let Some(type_str) = read_string_member(scope, data_obj, b"type") {
    let data_bytes = read_uint8array_member(scope, data_obj, b"data")
      .ok_or_else(|| type_error("Missing keyData.data".to_string()))?;
    let boxed = data_bytes.into_boxed_slice();
    return Ok(match type_str.as_str() {
      "secret" => RawKeyData::Secret(boxed),
      "private" => RawKeyData::Private(boxed),
      "public" => RawKeyData::Public(boxed),
      _ => RawKeyData::Raw(boxed),
    });
  }
  // SeededPrivate form.
  let pk = read_uint8array_member(scope, data_obj, b"privateKey")
    .ok_or_else(|| type_error("Missing keyData.privateKey".to_string()))?;
  let seed = read_uint8array_member(scope, data_obj, b"seed");
  Ok(RawKeyData::SeededPrivate {
    seed: seed.map(|s| s.into_boxed_slice()),
    private_key: pk.into_boxed_slice(),
  })
}

fn read_uint8array_member<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  obj: v8::Local<'s, v8::Object>,
  field: &[u8],
) -> Option<Vec<u8>> {
  let k = v8::String::new_from_one_byte(
    scope,
    field,
    v8::NewStringType::Internalized,
  )?;
  let v = obj.get(scope, k.into())?;
  if v.is_undefined() || v.is_null() {
    return None;
  }
  let view = v8::Local::<v8::ArrayBufferView>::try_from(v).ok()?;
  let mut out = vec![0u8; view.byte_length()];
  let n = view.copy_contents(&mut out);
  out.truncate(n);
  Some(out)
}
