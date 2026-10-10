//! Shared favorites: one set on disk (app-data dir) used by both the desktop
//! UI (Tauri commands) and the phone UI (LAN `/api/favorites`). Either side
//! can toggle and the other sees it; the JSON file survives restarts.
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct FavStore {
    inner: Arc<Mutex<HashSet<String>>>,
    path: PathBuf,
}

impl FavStore {
    pub fn load(dir: PathBuf) -> Self {
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("favorites.json");
        let set: HashSet<String> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            inner: Arc::new(Mutex::new(set)),
            path,
        }
    }

    pub fn list(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .inner
            .lock()
            .map(|g| g.iter().cloned().collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Insert/remove one entry, persist, return the full updated list.
    pub fn set(&self, file: String, fav: bool) -> Vec<String> {
        let v = {
            let mut g = match self.inner.lock() {
                Ok(g) => g,
                Err(_) => return self.list(),
            };
            if fav {
                g.insert(file);
            } else {
                g.remove(&file);
            }
            let mut v: Vec<String> = g.iter().cloned().collect();
            v.sort();
            v
        };
        if let Ok(json) = serde_json::to_string(&v) {
            let _ = std::fs::write(&self.path, json);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_persist() {
        let dir = std::env::temp_dir().join(format!("loopz-favtest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = FavStore::load(dir.clone());
        assert!(s.list().is_empty());
        s.set("loops/amen.opus".into(), true);
        s.set("loops/bb.opus".into(), true);
        s.set("loops/amen.opus".into(), false);
        assert_eq!(s.list(), vec!["loops/bb.opus".to_string()]);
        // A fresh store on the same dir sees the persisted file.
        assert_eq!(FavStore::load(dir.clone()).list(), vec!["loops/bb.opus".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
