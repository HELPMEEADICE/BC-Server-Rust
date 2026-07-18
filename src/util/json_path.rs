//! Helpers for client AccountUpdate payloads that use Mongo-style dotted paths
//! (e.g. `ExtensionSettings.UndergroundPrison`).

use serde_json::{Map, Value};

/// Apply a single dotted path update into a JSON object root.
///
/// - `"Money"` → root["Money"] = value
/// - `"ExtensionSettings.UndergroundPrison"` → root["ExtensionSettings"]["UndergroundPrison"] = value
///
/// Intermediate objects are created as needed. If an intermediate value is not
/// an object, it is replaced with a new object.
pub fn apply_dotted_path(root: &mut Map<String, Value>, path: &str, value: Value) {
    let segments: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return;
    }
    if segments.len() == 1 {
        root.insert(segments[0].to_string(), value);
        return;
    }

    let mut current = root;
    for seg in &segments[..segments.len() - 1] {
        let key = (*seg).to_string();
        let needs_obj = match current.get(&key) {
            Some(Value::Object(_)) => false,
            _ => true,
        };
        if needs_obj {
            current.insert(key.clone(), Value::Object(Map::new()));
        }
        current = current
            .get_mut(&key)
            .and_then(|v| v.as_object_mut())
            .expect("just ensured object");
    }
    current.insert(segments[segments.len() - 1].to_string(), value);
}

/// Merge a set of field updates (possibly dotted) into an account JSON object.
/// Skips `_id`, `MapData`, and `AccountName` (identity).
pub fn merge_set_into_object(root: &mut Map<String, Value>, set: &Map<String, Value>) {
    for (k, v) in set {
        if k == "_id" || k == "MapData" || k == "AccountName" {
            continue;
        }
        if k.contains('.') {
            apply_dotted_path(root, k, v.clone());
        } else {
            root.insert(k.clone(), v.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dotted_extension_settings_nested() {
        let mut root = Map::new();
        root.insert("ExtensionSettings".into(), json!({ "Other": 1 }));
        apply_dotted_path(
            &mut root,
            "ExtensionSettings.UndergroundPrison",
            json!({ "Owned": true, "Vault": 100 }),
        );
        assert_eq!(
            root.get("ExtensionSettings")
                .and_then(|v| v.get("Other"))
                .and_then(|v| v.as_i64()),
            Some(1)
        );
        assert_eq!(
            root.get("ExtensionSettings")
                .and_then(|v| v.get("UndergroundPrison"))
                .and_then(|v| v.get("Owned"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        // Must not leave a literal dotted key
        assert!(!root.contains_key("ExtensionSettings.UndergroundPrison"));
    }

    #[test]
    fn creates_missing_parent() {
        let mut root = Map::new();
        apply_dotted_path(&mut root, "ExtensionSettings.UndergroundPrison", json!(42));
        assert_eq!(root["ExtensionSettings"]["UndergroundPrison"], json!(42));
    }

    #[test]
    fn plain_key_still_works() {
        let mut root = Map::new();
        apply_dotted_path(&mut root, "Money", json!(500));
        assert_eq!(root.get("Money").and_then(|v| v.as_i64()), Some(500));
    }
}
