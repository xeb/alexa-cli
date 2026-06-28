use crate::config::Config;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

pub fn key_for(mp3: &[u8]) -> String {
    blake3::hash(mp3).to_hex().to_string()
}

pub struct Cache {
    path: PathBuf,
    map: HashMap<String, String>,
}

impl Cache {
    pub fn load() -> Cache {
        let path = Config::dir().join("cache.json");
        let map = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Cache { path, map }
    }
    pub fn get(&self, key: &str) -> Option<String> {
        self.map.get(key).cloned()
    }
    pub fn put(&mut self, key: &str, transcript: &str) -> Result<()> {
        self.map.insert(key.to_string(), transcript.to_string());
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(&self.map)?)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn empty_for_test() -> Cache {
        Cache { path: PathBuf::from("/dev/null"), map: HashMap::new() }
    }
    #[cfg(test)]
    pub fn put_mem(&mut self, key: &str, transcript: &str) {
        self.map.insert(key.to_string(), transcript.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_stable_and_content_addressed() {
        let a = key_for(b"hello");
        let b = key_for(b"hello");
        let c = key_for(b"world");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64); // blake3 hex
    }

    #[test]
    fn put_then_get_roundtrips_in_memory() {
        let mut cache = Cache::empty_for_test();
        assert_eq!(cache.get("k"), None);
        cache.put_mem("k", "the time is noon");
        assert_eq!(cache.get("k"), Some("the time is noon".to_string()));
    }
}
