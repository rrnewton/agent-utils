//! Files that one agentctl writes and another, older or newer, reads (#809
//! chat-bridge-json-forward-compat).
//!
//! A newer agentctl may add a key anywhere in a persisted record. An older one reading the file
//! must not refuse it: it reads what it knows and warns once per file and set of keys. A record an
//! older build may rewrite (the bridge configuration, an agent record, `restore.json`,
//! `profiles.json`, `event-sinks.json`) carries a [`KeptKeys`] catch-all, so the key survives the
//! rewrite. A record written once and never rewritten by a reader just ignores it.
//!
//! [`decode`] finds both kinds. A kept key is named by its catch-all as it is read. An ignored
//! key is found by comparing the document with the record written back out: a key the document
//! holds that the record does not write is one this build dropped. A key whose value is `null`,
//! `false`, zero, or empty is not named, because that is what a field marked
//! `skip_serializing_if` leaves out, so an older file that wrote a default explicitly raises no
//! false alarm; and a new key a newer build writes only when it is on is never one of those.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

thread_local! {
    /// The keys the catch-alls in the record being decoded on this thread kept.
    static KEPT: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// The keys of one record that this build does not know, kept so that writing the record back
/// out keeps them. Flatten it into a record (`#[serde(flatten)] unknown: KeptKeys`); an empty one
/// writes nothing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeptKeys(pub BTreeMap<String, Value>);

impl KeptKeys {
    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for KeptKeys {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for KeptKeys {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let keys = BTreeMap::<String, Value>::deserialize(deserializer)?;
        KEPT.with(|kept| {
            if let Some(kept) = kept.borrow_mut().as_mut() {
                kept.extend(keys.keys().cloned());
            }
        });
        Ok(Self(keys))
    }
}

/// Decode `value` as `T`, and name the keys it holds that this build does not know: the ones a
/// catch-all kept, by name, and the ones `T` dropped, by dotted path (`config.labels.extra`,
/// `entries[2].note`).
///
/// # Errors
///
/// `value` does not satisfy `T`'s serde contract, or `T` cannot be written back out.
pub fn decode<T: DeserializeOwned + Serialize>(
    value: Value,
) -> serde_json::Result<(T, Vec<String>)> {
    let (decoded, mut unknown) = decode_kept::<T>(&value)?;
    dropped_keys(&value, &serde_json::to_value(&decoded)?, "", &mut unknown);
    Ok((decoded, unknown))
}

/// Decode `value` as `T`, and name the keys its catch-alls kept. For a record whose every object
/// that may gain a key carries a [`KeptKeys`], this names them all, and `T` need not be written
/// back out.
///
/// # Errors
///
/// `value` does not satisfy `T`'s serde contract.
pub fn decode_kept<T: DeserializeOwned>(value: &Value) -> serde_json::Result<(T, Vec<String>)> {
    let outer = KEPT.with(|kept| kept.replace(Some(Vec::new())));
    let decoded = T::deserialize(value);
    let unknown = KEPT.with(|kept| kept.replace(outer)).unwrap_or_default();
    Ok((decoded?, unknown))
}

/// Add to `found` the path of each key in `document` that `written` lacks, below `prefix`.
fn dropped_keys(document: &Value, written: &Value, prefix: &str, found: &mut Vec<String>) {
    match (document, written) {
        (Value::Object(document), Value::Object(written)) => {
            for (key, value) in document {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                match written.get(key) {
                    Some(written) => dropped_keys(value, written, &path, found),
                    None if !left_out_by_default(value) => found.push(path),
                    None => {}
                }
            }
        }
        (Value::Array(document), Value::Array(written)) if document.len() == written.len() => {
            for (index, (value, written)) in document.iter().zip(written).enumerate() {
                dropped_keys(value, written, &format!("{prefix}[{index}]"), found);
            }
        }
        _ => {}
    }
}

/// Whether `value` is one a field marked `skip_serializing_if` leaves out.
fn left_out_by_default(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(value) => !value,
        Value::Number(number) => number.as_f64() == Some(0.0),
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(entries) => entries.is_empty(),
    }
}

/// Print, once per process for each file and set of keys, that `path` holds `keys` this build
/// does not know and reads around.
pub fn warn_unknown(path: &Path, keys: &[String]) {
    static WARNED: std::sync::Mutex<BTreeSet<String>> = std::sync::Mutex::new(BTreeSet::new());
    if keys.is_empty() {
        return;
    }
    let mut keys = keys.to_vec();
    keys.sort();
    keys.dedup();
    let warning = format!(
        "agentctl: warning: {} has key(s) this agentctl does not know: {}; reading around them \
         (a newer agentctl may use them)",
        path.display(),
        crate::plugins::key_list(&keys)
    );
    let mut warned = WARNED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if warned.insert(warning.clone()) {
        eprintln!("{warning}");
    }
}

/// [`decode`] the JSON `bytes` read from `path`, warning once about keys this build does not
/// know.
///
/// # Errors
///
/// `bytes` is not JSON, or does not satisfy `T`'s serde contract.
pub fn decode_file<T: DeserializeOwned + Serialize>(
    path: &Path,
    bytes: &[u8],
) -> serde_json::Result<T> {
    let (decoded, unknown) = decode(serde_json::from_slice(bytes)?)?;
    warn_unknown(path, &unknown);
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct Inner {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct Ignoring {
        inner: Inner,
        list: Vec<Inner>,
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct Keeping {
        name: String,
        #[serde(flatten)]
        unknown: KeptKeys,
    }

    #[test]
    fn keys_a_record_drops_are_named_by_path_and_defaults_are_not() {
        let (decoded, unknown) = decode::<Ignoring>(serde_json::json!({
            "inner": {"name": "a", "note": null, "added": 3},
            "list": [{"name": "b"}, {"name": "c", "later": {"x": 1}}],
            "top": "new",
            "off": false,
        }))
        .expect("decoded");
        assert_eq!(decoded.inner.name, "a");
        assert_eq!(unknown, ["inner.added", "list[1].later", "top"]);
    }

    #[test]
    fn keys_a_catch_all_keeps_are_named_and_written_back() {
        let (decoded, unknown) =
            decode::<Keeping>(serde_json::json!({"name": "a", "newer": {"on": true}}))
                .expect("decoded");
        assert_eq!(unknown, ["newer"]);
        assert_eq!(
            serde_json::to_value(&decoded).expect("written"),
            serde_json::json!({"name": "a", "newer": {"on": true}})
        );
        let (_, unknown) = decode::<Keeping>(serde_json::json!({"name": "a"})).expect("decoded");
        assert!(unknown.is_empty());
    }
}
