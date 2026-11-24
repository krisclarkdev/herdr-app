//! Named UI configurations: device-local snapshots of every machine Document + View.

use crate::{mirror::Document, persist};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const FILE: &str = "ui-configs.json";
/// Refuse a single machine document that would exceed core's ~1 MiB JSON cap.
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    #[serde(default)]
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub saved_at_ms: u64,
    pub view: Value,
    /// Machine name → Document JSON. `""` is the local machine.
    pub machines: BTreeMap<String, Value>,
    /// Fields a newer build may add.
    #[serde(flatten)]
    pub rest: serde_json::Map<String, Value>,
}

impl Store {
    pub fn path() -> PathBuf {
        crate::mods::app_dir().join(FILE)
    }

    pub fn load() -> Self {
        let Ok(text) = std::fs::read_to_string(Self::path()) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let body = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into()) + "\n";
        persist::write_atomic(&Self::path(), body)
    }

    pub fn find_mut(&mut self, name_or_id: &str) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.name == name_or_id || i.id == name_or_id)
    }

    pub fn find(&self, name_or_id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.name == name_or_id || i.id == name_or_id)
    }

    pub fn names(&self) -> Vec<String> {
        self.items.iter().map(|i| i.name.clone()).collect()
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn fresh_id() -> String {
    let mut h = DefaultHasher::new();
    now_ms().hash(&mut h);
    std::process::id().hash(&mut h);
    format!("cfg_{:016x}", h.finish())
}

/// Serialize a document and refuse oversized payloads before they hit core.
pub fn document_value(doc: &Document) -> Result<Value, String> {
    let value = serde_json::to_value(doc).map_err(|e| format!("Cannot encode document: {e}"))?;
    ensure_document_size(&value)?;
    Ok(value)
}

pub fn ensure_document_size(value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| format!("Cannot measure document: {e}"))?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "Configuration document is {} bytes (limit {MAX_DOCUMENT_BYTES}); shrink spaces before saving",
            bytes.len()
        ));
    }
    Ok(())
}

pub fn document_from_value(value: &Value) -> Result<Document, String> {
    serde_json::from_value(value.clone()).map_err(|e| format!("Corrupt configuration document: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mirror::Document;

    #[test]
    fn store_round_trips_and_finds_by_name_or_id() {
        let dir = std::env::temp_dir().join(format!("herdr-ui-config-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var_os("HERDR_APP_HOME");
        unsafe { std::env::set_var("HERDR_APP_HOME", &dir) };

        let mut store = Store::default();
        let item = Item {
            id: "cfg_abc".into(),
            name: "Morning".into(),
            saved_at_ms: 1,
            view: serde_json::json!({}),
            machines: BTreeMap::from([("".into(), serde_json::json!({"spaces":[]}))]),
            rest: Default::default(),
        };
        store.items.push(item);
        store.active = Some("Morning".into());
        store.save().unwrap();

        let loaded = Store::load();
        assert_eq!(loaded.active.as_deref(), Some("Morning"));
        assert_eq!(loaded.find("Morning").unwrap().id, "cfg_abc");
        assert_eq!(loaded.find("cfg_abc").unwrap().name, "Morning");
        assert_eq!(loaded.names(), vec!["Morning".to_string()]);

        match prev {
            Some(v) => unsafe { std::env::set_var("HERDR_APP_HOME", v) },
            None => unsafe { std::env::remove_var("HERDR_APP_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_document_fits_and_decodes() {
        let v = document_value(&Document::default()).unwrap();
        assert_eq!(document_from_value(&v).unwrap(), Document::default());
    }

    #[test]
    fn oversized_document_is_refused() {
        let value = serde_json::json!({ "spaces": [], "pad": "x".repeat(MAX_DOCUMENT_BYTES) });
        assert!(ensure_document_size(&value).is_err());
        assert!(ensure_document_size(&serde_json::json!({"spaces": []})).is_ok());
    }
}
