//! Catalog of the published PLaMo 3 Translate GGUF files and the local model store.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Model size (total parameters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModelSize {
    B0_8,
    B2,
    B8,
    B31,
}

impl ModelSize {
    pub const fn label(self) -> &'static str {
        match self {
            Self::B0_8 => "0.8B",
            Self::B2 => "2B",
            Self::B8 => "8B",
            Self::B31 => "31B",
        }
    }
}

/// GGUF quantization type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Quant {
    Q4KM,
    Q4_0,
    Q3KM,
    Q2K,
}

impl Quant {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Q4KM => "Q4_K_M",
            Self::Q4_0 => "Q4_0",
            Self::Q3KM => "Q3_K_M",
            Self::Q2K => "Q2_K",
        }
    }
}

/// One downloadable GGUF file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CatalogEntry {
    pub size: ModelSize,
    pub quant: Quant,
    /// Hugging Face repository, e.g. `pfnet/plamo-3-translate-2b-GGUF`.
    pub repo: &'static str,
    /// Pinned repository commit, so the URL always serves the file `sha256` describes.
    pub revision: &'static str,
    pub file_name: &'static str,
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: &'static str,
}

impl CatalogEntry {
    /// Stable identifier: the file name without the `.gguf` extension.
    pub fn id(&self) -> &'static str {
        self.file_name
            .strip_suffix(".gguf")
            .unwrap_or(self.file_name)
    }

    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.revision, self.file_name
        )
    }

    /// Whether this is the recommended quantization for its size.
    pub fn is_recommended_quant(&self) -> bool {
        self.quant == Quant::Q4KM
    }
}

impl fmt::Display for CatalogEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} · {}", self.size.label(), self.quant.label())
    }
}

macro_rules! entry {
    ($size:ident, $slug:literal, $quant:ident, $q:literal, $rev:literal, $bytes:literal, $sha:literal) => {
        CatalogEntry {
            size: ModelSize::$size,
            quant: Quant::$quant,
            repo: concat!("pfnet/plamo-3-translate-", $slug, "-GGUF"),
            revision: $rev,
            file_name: concat!("plamo-3-translate-", $slug, "-", $q, ".gguf"),
            bytes: $bytes,
            sha256: $sha,
        }
    };
}

/// All published GGUF files, ordered by size and then by quality.
#[rustfmt::skip]
pub const CATALOG: &[CatalogEntry] = &[
    entry!(B0_8, "0.8b", Q4KM, "Q4_K_M", "03891e907db8a43c97d3a7c75add125cc6d1f96a", 573116288, "3dbb09295bfd3350231ba0dba92bc72cb14111ef1b034e37dad39d581bd86b9c"),
    entry!(B0_8, "0.8b", Q4_0, "Q4_0",   "03891e907db8a43c97d3a7c75add125cc6d1f96a", 547532672, "2805c14de045d50f98e35e81aaf0fd23f6baa49aa8e06efcc11c46d44021bd05"),
    entry!(B0_8, "0.8b", Q3KM, "Q3_K_M", "03891e907db8a43c97d3a7c75add125cc6d1f96a", 486375296, "c6e6a9d0b05f3c0a7a4b70c6460a68dcf169d417984024b761c50e42e5713cdb"),
    entry!(B0_8, "0.8b", Q2K,  "Q2_K",   "03891e907db8a43c97d3a7c75add125cc6d1f96a", 410128256, "30ae7b485e0f54980c4a5223d8415adbe04c382b36861687361b2b492929cc16"),
    entry!(B2,   "2b",   Q4KM, "Q4_K_M", "f2b4599dff9b3e45c62567eac15e83186b4cb64e", 1807192480, "2c288ecc7ebfe60c3561726580da3583767f3c41ca3ef5c6825bf6c4cc060787"),
    entry!(B2,   "2b",   Q4_0, "Q4_0",   "f2b4599dff9b3e45c62567eac15e83186b4cb64e", 1698566560, "7a1839ed7f315b03176a7c7bb697629a95f8c3a61e669dfdf1cbeb6471551a66"),
    entry!(B2,   "2b",   Q3KM, "Q3_K_M", "f2b4599dff9b3e45c62567eac15e83186b4cb64e", 1495876000, "a940db3afd6749bc74e550e324cccf1beb639cbf2dfe449f9a7f143e696fbec5"),
    entry!(B2,   "2b",   Q2K,  "Q2_K",   "f2b4599dff9b3e45c62567eac15e83186b4cb64e", 1225601440, "a2a02331f4922d2e283ac83c45cdbe2ab0faaf880e613078cde1299d7be78e2a"),
    entry!(B8,   "8b",   Q4KM, "Q4_K_M", "c420296e9694ccb2455f038b3f29eee5a2200217", 5280435168, "6b429549113ce0a61281ac72be195697241f89266f4247efec4893e4f580cf10"),
    entry!(B8,   "8b",   Q3KM, "Q3_K_M", "c420296e9694ccb2455f038b3f29eee5a2200217", 4310207456, "2413a3065f8666303d13b39161936786f2d8db9c4f1de4c2d2fcf80a72652fdc"),
    entry!(B8,   "8b",   Q2K,  "Q2_K",   "c420296e9694ccb2455f038b3f29eee5a2200217", 3450735584, "610bd828dfe6f372437eb35b9be61fefeaa1aaa7861bce8446009f09f1545d3c"),
    entry!(B31,  "31b",  Q4KM, "Q4_K_M", "d0df961b923db3ff9fc325f4410d6f09005afcd6", 19526583392, "d1ed08149f888ef7a0da38e26bf394a13d0f079b592229c8778f5f1a91cf6457"),
    entry!(B31,  "31b",  Q3KM, "Q3_K_M", "d0df961b923db3ff9fc325f4410d6f09005afcd6", 15863120992, "f7b5ff4ce1f893acf029a4b00360bb59898399162576c1d1bbcd587b13c9553a"),
    entry!(B31,  "31b",  Q2K,  "Q2_K",   "d0df961b923db3ff9fc325f4410d6f09005afcd6", 12609913952, "3565eb103a56cd7d8a3229f44a33b2bd601bcfa71018a4eed9f6648629ddaf03"),
];

/// The catalog entry used when nothing else is known about the machine.
pub fn default_entry() -> &'static CatalogEntry {
    find_entry("plamo-3-translate-2b-Q4_K_M").expect("default model is in the catalog")
}

pub fn find_entry(id: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|e| e.id() == id)
}

/// Picks the recommended model for the given amount of memory available to the model.
///
/// The model file plus working memory should fit with some headroom, so this asks for
/// roughly 1.5x the file size.
pub fn recommend(available_bytes: u64) -> &'static CatalogEntry {
    let fits = |e: &&CatalogEntry| e.bytes.saturating_mul(3) / 2 <= available_bytes;
    CATALOG
        .iter()
        .filter(|e| e.is_recommended_quant() && e.size <= ModelSize::B8)
        .filter(fits)
        .max_by_key(|e| e.size)
        .unwrap_or_else(|| {
            CATALOG
                .iter()
                .find(|e| e.size == ModelSize::B0_8 && e.is_recommended_quant())
                .expect("0.8B Q4_K_M is in the catalog")
        })
}

/// Local state of a catalog entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalState {
    /// The verified file is present.
    Downloaded,
    /// A partial download of this many bytes can be resumed.
    Partial(u64),
    Missing,
}

/// Directory holding downloaded models.
#[derive(Debug, Clone)]
pub struct ModelStore {
    dir: PathBuf,
}

impl ModelStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Final location of a downloaded model.
    pub fn path(&self, entry: &CatalogEntry) -> PathBuf {
        self.dir.join(entry.file_name)
    }

    /// Location of an in-progress download.
    pub fn partial_path(&self, entry: &CatalogEntry) -> PathBuf {
        self.dir.join(format!("{}.part", entry.file_name))
    }

    /// Files only reach their final name after verification, so a final file of the right
    /// size counts as downloaded.
    pub fn state(&self, entry: &CatalogEntry) -> LocalState {
        if std::fs::metadata(self.path(entry)).is_ok_and(|m| m.len() == entry.bytes) {
            return LocalState::Downloaded;
        }
        match std::fs::metadata(self.partial_path(entry)) {
            Ok(m) if m.len() > 0 => LocalState::Partial(m.len()),
            _ => LocalState::Missing,
        }
    }

    /// Deletes the model and any partial download.
    pub fn delete(&self, entry: &CatalogEntry) -> io::Result<()> {
        for path in [self.path(entry), self.partial_path(entry)] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        assert_eq!(CATALOG.len(), 14);
        for e in CATALOG {
            assert!(e.file_name.starts_with("plamo-3-translate-"));
            assert!(e.file_name.contains(e.quant.label()), "{}", e.file_name);
            assert_eq!(e.sha256.len(), 64);
            assert!(e.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert_eq!(e.revision.len(), 40);
            assert_eq!(find_entry(e.id()), Some(e));
        }
        assert_eq!(default_entry().to_string(), "2B · Q4_K_M");
    }

    #[test]
    fn url_points_to_pinned_revision() {
        let e = default_entry();
        assert_eq!(
            e.url(),
            "https://huggingface.co/pfnet/plamo-3-translate-2b-GGUF/resolve/\
             f2b4599dff9b3e45c62567eac15e83186b4cb64e/plamo-3-translate-2b-Q4_K_M.gguf"
        );
    }

    #[test]
    fn recommends_by_available_memory() {
        const GB: u64 = 1 << 30;
        assert_eq!(recommend(16 * GB).size, ModelSize::B8);
        assert_eq!(recommend(4 * GB).size, ModelSize::B2);
        assert_eq!(recommend(GB).size, ModelSize::B0_8);
        assert_eq!(recommend(0).size, ModelSize::B0_8);
        // 31B is never recommended automatically.
        assert_eq!(recommend(128 * GB).size, ModelSize::B8);
    }

    #[test]
    fn store_reports_local_state() {
        let dir = std::env::temp_dir().join(format!("plamo-store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = ModelStore::new(&dir);
        let e = default_entry();
        assert_eq!(store.state(e), LocalState::Missing);
        std::fs::write(store.partial_path(e), b"abc").unwrap();
        assert_eq!(store.state(e), LocalState::Partial(3));
        // A final file of the wrong size does not count.
        std::fs::write(store.path(e), b"short").unwrap();
        assert_eq!(store.state(e), LocalState::Partial(3));
        store.delete(e).unwrap();
        assert_eq!(store.state(e), LocalState::Missing);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
