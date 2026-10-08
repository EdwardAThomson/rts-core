//! Where a game's files come from: a folder on disk, or files already in memory. The browser has no file system,
//! so the web build fetches what it needs first and hands it over as memory.

use std::collections::BTreeMap;
use std::path::PathBuf;

pub enum Files {
    /// Files under a folder, read when asked for.
    Dir(PathBuf),
    /// Files fetched ahead of time, by `/`-separated path. `label` says where they came from, for messages.
    Memory { label: String, files: BTreeMap<String, Vec<u8>> },
}

impl Files {
    /// The bytes of `path`, given relative to the folder (or to where the memory files were fetched from).
    pub fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        match self {
            Files::Dir(dir) => {
                let full = dir.join(path);
                std::fs::read(&full).map_err(|e| format!("{}: {e}", full.display()))
            }
            Files::Memory { label, files } => {
                files.get(path).cloned().ok_or_else(|| format!("{label}/{path}: missing"))
            }
        }
    }

    /// Where `path` is, for messages.
    pub fn name(&self, path: &str) -> String {
        match self {
            Files::Dir(dir) => dir.join(path).display().to_string(),
            Files::Memory { label, .. } => format!("{label}/{path}"),
        }
    }

    pub fn read_text(&self, path: &str) -> Result<String, String> {
        String::from_utf8(self.read(path)?).map_err(|e| format!("{path}: {e}"))
    }
}
