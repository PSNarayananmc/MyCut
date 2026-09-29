//! Cache: content-hash-keyed JSON cache with LRU eviction and size cap.
//! Never touches original media. Keys are `sha256(content_hash, params)`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Deterministic cache rooted at `dir`.
pub struct Cache {
    dir: PathBuf,
    max_bytes: u64,
}

impl Cache {
    #[must_use]
    pub fn new(dir: &Path, max_bytes: u64) -> Self {
        let _ = fs::create_dir_all(dir);
        Self {
            dir: dir.to_path_buf(),
            max_bytes,
        }
    }

    /// Stable key from content hash + named parameters.
    #[must_use]
    pub fn key(content_hash: &str, params: &BTreeMap<String, String>) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(content_hash.as_bytes());
        for (k, v) in params {
            h.update(k.as_bytes());
            h.update(b"=");
            h.update(v.as_bytes());
            h.update(b";");
        }
        hex::encode(h.finalize())
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    /// Read a cached JSON value.
    #[must_use]
    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        let p = self.path(key);
        let bytes = fs::read(&p).ok()?;
        // Touch for LRU (mtime update is best-effort).
        let _ = fs::File::open(&p).and_then(|f| {
            f.set_times(
                std::fs::FileTimes::new()
                    .set_accessed(std::time::SystemTime::now())
                    .set_modified(std::time::SystemTime::now()),
            )
        });
        serde_json::from_slice(&bytes).ok()
    }

    /// Write a JSON value.
    ///
    /// # Errors
    /// IO failures are surfaced; callers may treat them as non-fatal.
    pub fn put<T: serde::Serialize>(&self, key: &str, value: &T) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
        let tmp = self.dir.join(format!("{key}.tmp"));
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, self.path(key))?;
        self.evict_if_needed();
        Ok(())
    }

    /// Total cache size in bytes.
    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        walk(&self.dir).unwrap_or(0)
    }

    /// Remove all entries.
    pub fn clear(&self) {
        let _ = fs::remove_dir_all(&self.dir);
        let _ = fs::create_dir_all(&self.dir);
    }

    /// LRU eviction: delete oldest-accessed entries until under cap.
    pub fn evict_if_needed(&self) {
        let entries = match fs::read_dir(&self.dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .filter_map(|e| {
                    let meta = e.metadata().ok()?;
                    let modified = e.metadata().and_then(|m| m.modified()).ok()?;
                    Some((e.path(), meta.len(), modified))
                })
                .collect::<Vec<_>>(),
            Err(_) => return,
        };
        let total: u64 = entries.iter().map(|(_, l, _)| l).sum();
        if total <= self.max_bytes {
            return;
        }
        let mut by_age = entries;
        by_age.sort_by_key(|(_, _, m)| *m);
        let mut total_now = total;
        for (path, len, _) in by_age {
            if total_now <= self.max_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total_now = total_now.saturating_sub(len);
            }
        }
    }
}

fn walk(dir: &Path) -> std::io::Result<u64> {
    let mut total = 0;
    for e in fs::read_dir(dir)? {
        let e = e?;
        let meta = e.metadata()?;
        if meta.is_dir() {
            total += walk(&e.path())?;
        } else {
            total += meta.len();
        }
    }
    Ok(total)
}

/// Content hash used for cache keys and relink: sha256 over size + first and
/// last 1 MiB windows (fast on large files, stable across runs).
///
/// # Errors
/// IO failures surfaced.
pub fn content_hash(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Seek, SeekFrom};
    let meta = fs::metadata(path)?;
    let mut hasher = Sha256::new();
    hasher.update(format!("size:{}", meta.len()));
    let mut f = fs::File::open(path)?;
    let window: u64 = 1024 * 1024;
    let mut head = vec![0u8; window.min(meta.len()) as usize];
    f.read_exact(&mut head)?;
    hasher.update(&head);
    if meta.len() > window {
        f.seek(SeekFrom::Start(meta.len() - window))?;
        let mut tail = vec![0u8; window as usize];
        f.read_exact(&mut tail)?;
        hasher.update(&tail);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn keys_stable_and_param_sensitive() {
        let a = Cache::key("hash1", &params(&[("fps", "5")]));
        let b = Cache::key("hash1", &params(&[("fps", "5")]));
        let c = Cache::key("hash1", &params(&[("fps", "10")]));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn put_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 10_000_000);
        let key = Cache::key("h", &params(&[("x", "1")]));
        cache
            .put(&key, &serde_json::json!({"scenes": [1, 2]}))
            .unwrap();
        let got: Option<serde_json::Value> = cache.get(&key);
        assert_eq!(got, Some(serde_json::json!({"scenes": [1, 2]})));
    }

    #[test]
    fn eviction_respects_cap() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 4_000);
        for i in 0..10 {
            let key = Cache::key(&format!("h{i}"), &params(&[]));
            cache.put(&key, &vec![0u8; 1000]).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5)); // distinct mtimes
        }
        assert!(
            cache.size_bytes() < 5_500,
            "cache must evict under cap, got {}",
            cache.size_bytes()
        );
    }

    #[test]
    fn clear_empties() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 10_000);
        let key = Cache::key("h", &params(&[]));
        cache.put(&key, &"data").unwrap();
        assert!(cache.size_bytes() > 0);
        cache.clear();
        assert_eq!(cache.size_bytes(), 0);
    }
}
