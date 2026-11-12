//! Vanilla's languages, fetched when the player picks one: the app ships only `en_us` of the
//! 142 lang files (about 85 MB together; `xtask ios-pack`). The others come from Mojang's asset
//! server, as the official launcher fetches them, by the hash the prepared `asset-index.json`
//! gives, and stay in the app's caches (iOS may empty them: they are fetched again).
use horizon_client::lang::ResourceSource;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Mojang's asset server: objects by their SHA-1, `<first two hex digits>/<hash>`.
const OBJECTS: &str = "https://resources.download.minecraft.net";

/// The lang files the asset index has, and where fetched ones are kept.
pub struct Languages {
    /// `assets/minecraft/lang/<code>.json` → its SHA-1.
    hashes: HashMap<String, String>,
    cache: PathBuf,
    /// Codes whose fetch failed this run: not asked again until the next start.
    failed: Mutex<Vec<String>>,
}

impl Languages {
    /// The lang files of `resources`' asset index, cached under `cache`.
    pub fn new(resources: &Path, cache: PathBuf) -> Self {
        let mut hashes = HashMap::new();
        let index = horizon_core::prepared_files::read(&resources.join("asset-index.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        for (name, object) in index
            .as_ref()
            .and_then(|i| i["objects"].as_object())
            .into_iter()
            .flatten()
        {
            if let (Some(code), Some(hash)) = (
                name.strip_prefix("minecraft/lang/"),
                object["hash"].as_str(),
            ) {
                hashes.insert(format!("assets/minecraft/lang/{code}"), hash.to_owned());
            }
        }
        Self {
            hashes,
            cache,
            failed: Mutex::new(Vec::new()),
        }
    }

    /// The cached file of `hash`, checked; else fetched, checked and cached.
    fn fetch(&self, path: &str, hash: &str) -> Option<Vec<u8>> {
        let cached = self.cache.join(hash);
        if let Ok(bytes) = std::fs::read(&cached)
            && sha1_hex(&bytes) == hash
        {
            return Some(bytes);
        }
        if self.failed.lock().ok()?.iter().any(|p| p == path) {
            return None;
        }
        let url = format!("{OBJECTS}/{}/{hash}", &hash[..2]);
        let fetched = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(20))
            .build()
            .get(&url)
            .call()
            .map_err(|e| e.to_string())
            .and_then(|response| {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(
                    &mut std::io::Read::take(response.into_reader(), 16 << 20),
                    &mut bytes,
                )
                .map_err(|e| e.to_string())?;
                Ok(bytes)
            })
            .and_then(|bytes| {
                if sha1_hex(&bytes) == hash {
                    Ok(bytes)
                } else {
                    Err("its hash differs".into())
                }
            });
        match fetched {
            Ok(bytes) => {
                let _ = std::fs::create_dir_all(&self.cache);
                let partial = self.cache.join(format!("{hash}.part"));
                if std::fs::write(&partial, &bytes).is_ok() {
                    let _ = std::fs::rename(&partial, &cached);
                }
                log::info!("Horizon: fetched {path} ({} KB)", bytes.len() / 1024);
                Some(bytes)
            }
            Err(e) => {
                log::warn!("Horizon: {path} not fetched: {e}");
                if let Ok(mut failed) = self.failed.lock() {
                    failed.push(path.to_owned());
                }
                None
            }
        }
    }
}

impl ResourceSource for Languages {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        // en_us and deprecated.json ship in the app: the prepared root has them.
        let hash = self.hashes.get(path)?;
        if path.ends_with("/en_us.json") || path.ends_with("/deprecated.json") {
            return None;
        }
        self.fetch(path, hash)
    }
    fn exists(&self, path: &str) -> bool {
        self.hashes.contains_key(path)
    }
}

fn sha1_hex(bytes: &[u8]) -> String {
    use sha1::Digest;
    sha1::Sha1::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
