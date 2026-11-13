// Mush8 - keys.rs
// Constantin Clerc (c22dev) - 0.1
use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::path::Path;

#[derive(Default)]
pub struct Keys(HashMap<String, Vec<u8>>);

impl Keys {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut map = HashMap::new();
        for line in text.lines() {
            let Some((name, value)) = line.split_once('=') else { continue };
            if let Ok(bytes) = hex::decode(value.trim()) {
                map.insert(name.trim().to_ascii_lowercase(), bytes);
            }
        }
        Ok(Keys(map))
    }

    pub fn get<const N: usize>(&self, name: &str) -> Result<[u8; N]> {
        let v = self.0.get(name).ok_or_else(|| anyhow!("missing key {name}"))?;
        v.as_slice().try_into().map_err(|_| anyhow!("key {name} has wrong length"))
    }

    pub fn indexed<const N: usize>(&self, prefix: &str, idx: u8) -> Result<[u8; N]> {
        self.get(&format!("{prefix}_{idx:02x}"))
    }
}
