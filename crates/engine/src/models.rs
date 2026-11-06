//! Catalog of PLaMo Translate GGUF files, their published quality, and the local model store.
//!
//! The catalog starts with the files known when this version was built ([`BUILTIN`]). Entries
//! found on Hugging Face at run time (see [`crate::discovery`]) are added with [`register`]; they
//! may add new sizes or quantizations, or supersede a built-in file with a newer revision.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Model families whose prompt format (PFML3) this app speaks.
pub const SUPPORTED_FAMILIES: &[&str] = &["plamo-3-translate"];

/// One downloadable GGUF file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CatalogEntry {
    /// Model family, e.g. `plamo-3-translate`.
    pub family: &'static str,
    /// Size as written in file names, e.g. `2b` or `0.8b`.
    pub size: &'static str,
    /// GGUF quantization type, e.g. `Q4_K_M`.
    pub quant: &'static str,
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

    /// `family-size`, e.g. `plamo-3-translate-2b`: the unquantized model this file comes from.
    pub fn base_model(&self) -> String {
        format!("{}-{}", self.family, self.size)
    }

    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.revision, self.file_name
        )
    }

    /// Parameter count in billions, parsed from the size (`0.8b` → 0.8).
    pub fn params_billion(&self) -> f32 {
        self.size
            .trim_end_matches(['b', 'B'])
            .parse()
            .unwrap_or(0.0)
    }

    /// Size for display, e.g. `2B`.
    pub fn size_label(&self) -> String {
        self.size.to_uppercase()
    }

    /// Whether this is the recommended quantization for its size.
    pub fn is_recommended_quant(&self) -> bool {
        self.quant == "Q4_K_M"
    }

    /// Published translation quality of this file, if known.
    pub fn quality(&self) -> Option<&'static Quality> {
        QUALITY
            .iter()
            .find(|(id, _)| *id == self.id())
            .map(|(_, q)| q)
    }

    /// Published quality of the unquantized (BF16) model of the same size, if known.
    pub fn reference_quality(&self) -> Option<&'static Quality> {
        let base = self.base_model();
        QUALITY.iter().find(|(id, _)| *id == base).map(|(_, q)| q)
    }
}

impl fmt::Display for CatalogEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} · {}", self.size_label(), self.quant)
    }
}

/// Translation quality published on the model card (higher is better).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quality {
    /// PFN Translation Benchmark, English ↔ Japanese.
    pub pftb: f32,
    /// SaaS benchmark, English → Japanese.
    pub saas: f32,
    /// WMT24++ COMET-22, English → Japanese.
    pub wmt24pp: f32,
    /// FLORES+ chrF++, Japanese ↔ 14 languages.
    pub flores_ja: f32,
    /// FLORES+ chrF++, English ↔ 14 languages.
    pub flores_en: f32,
}

const fn q(pftb: f32, saas: f32, wmt24pp: f32, flores_ja: f32, flores_en: f32) -> Quality {
    Quality {
        pftb,
        saas,
        wmt24pp,
        flores_ja,
        flores_en,
    }
}

/// Scores from the "Accuracy changes from quantization" tables of the GGUF model cards.
/// Keys are file ids, plus `family-size` for the BF16 reference.
#[rustfmt::skip]
pub const QUALITY: &[(&str, Quality)] = &[
    ("plamo-3-translate-0.8b",        q(0.5862, 0.5530, 0.8724, 36.08, 54.44)),
    ("plamo-3-translate-0.8b-Q4_K_M", q(0.5832, 0.5451, 0.8716, 35.90, 54.47)),
    ("plamo-3-translate-0.8b-Q4_0",   q(0.5853, 0.5500, 0.8723, 34.97, 54.11)),
    ("plamo-3-translate-0.8b-Q3_K_M", q(0.5798, 0.5520, 0.8710, 35.32, 54.15)),
    ("plamo-3-translate-0.8b-Q2_K",   q(0.5620, 0.5397, 0.8659, 33.56, 51.15)),
    ("plamo-3-translate-2b",          q(0.6061, 0.5612, 0.8780, 37.02, 55.74)),
    ("plamo-3-translate-2b-Q4_K_M",   q(0.6062, 0.5602, 0.8774, 37.00, 55.76)),
    ("plamo-3-translate-2b-Q4_0",     q(0.6069, 0.5604, 0.8791, 36.12, 55.62)),
    ("plamo-3-translate-2b-Q3_K_M",   q(0.6053, 0.5644, 0.8789, 36.81, 55.59)),
    ("plamo-3-translate-2b-Q2_K",     q(0.5977, 0.5610, 0.8777, 36.45, 55.02)),
    ("plamo-3-translate-8b",          q(0.6119, 0.5750, 0.8805, 37.45, 56.22)),
    ("plamo-3-translate-8b-Q4_K_M",   q(0.6100, 0.5765, 0.8802, 37.39, 56.18)),
    ("plamo-3-translate-8b-Q3_K_M",   q(0.6019, 0.5707, 0.8826, 37.20, 55.93)),
    ("plamo-3-translate-8b-Q2_K",     q(0.5989, 0.5676, 0.8791, 36.54, 55.31)),
    ("plamo-3-translate-31b",         q(0.6244, 0.5786, 0.8799, 37.81, 56.61)),
    ("plamo-3-translate-31b-Q4_K_M",  q(0.6237, 0.5770, 0.8810, 37.84, 56.55)),
    ("plamo-3-translate-31b-Q3_K_M",  q(0.6223, 0.5780, 0.8813, 37.78, 56.54)),
    ("plamo-3-translate-31b-Q2_K",    q(0.6126, 0.5745, 0.8801, 37.36, 56.16)),
];

macro_rules! entry {
    ($slug:literal, $q:literal, $rev:literal, $bytes:literal, $sha:literal) => {
        CatalogEntry {
            family: "plamo-3-translate",
            size: $slug,
            quant: $q,
            repo: concat!("pfnet/plamo-3-translate-", $slug, "-GGUF"),
            revision: $rev,
            file_name: concat!("plamo-3-translate-", $slug, "-", $q, ".gguf"),
            bytes: $bytes,
            sha256: $sha,
        }
    };
}

/// The GGUF files published when this version was built (the revisions of 2026-11-06).
#[rustfmt::skip]
pub const BUILTIN: &[CatalogEntry] = &[
    entry!("0.8b", "Q4_K_M", "dbeb0856cb0b004b484f190bb71dd65876297013", 573123200, "923b2f28a9856de1b8b3b610a7e59ded0b14d007466fd5dd526e6a9ecaadafdd"),
    entry!("0.8b", "Q4_0",   "dbeb0856cb0b004b484f190bb71dd65876297013", 547539584, "39300577d714b108a509ca9d582ea7335ae2fa4504ee73d3358c00b4980fb32a"),
    entry!("0.8b", "Q3_K_M", "dbeb0856cb0b004b484f190bb71dd65876297013", 486382208, "712dbe94cdb6dffa08082472e74ee8c65e12e688ad9e76cb944a1b3699be208f"),
    entry!("0.8b", "Q2_K",   "dbeb0856cb0b004b484f190bb71dd65876297013", 410135168, "8f84478629c5d36ffe6a28f4de1bbe7f49da0faf53644614453d00ad63330bea"),
    entry!("2b",   "Q4_K_M", "232fbd7916519edecddfdae0beae7e76fe70453f", 1807199392, "f36d0813dd95573a852ab5d76e374c543632d9e292929e922bcdbf7e53b494e0"),
    entry!("2b",   "Q4_0",   "232fbd7916519edecddfdae0beae7e76fe70453f", 1698573472, "9b976e7857b65012cb98da4f4176203fb42fadf542057980d5329a93d3e4f8f8"),
    entry!("2b",   "Q3_K_M", "232fbd7916519edecddfdae0beae7e76fe70453f", 1495882912, "24ef0a97f1527d2aa49fc12ddcd9eb6de74d6b883ffcd84c72cac299fafb3006"),
    entry!("2b",   "Q2_K",   "232fbd7916519edecddfdae0beae7e76fe70453f", 1225608352, "01517813751c20412568f93845858f6902eefc3794f247dfbb50bad466b108e7"),
    entry!("8b",   "Q4_K_M", "2234f168757f570714b333049c9d018161ec5277", 5280442080, "47a04e86a67bcc352fa186f01159060aae594b73993e1d49dbfc0ef0081d01ad"),
    entry!("8b",   "Q3_K_M", "2234f168757f570714b333049c9d018161ec5277", 4310214368, "b52b510f7bf1d49bff10cc9024518bfd88b78f0bb38d63956f5e7784f573d97b"),
    entry!("8b",   "Q2_K",   "2234f168757f570714b333049c9d018161ec5277", 3450742496, "917b86aedab74a3d2172bbed89830a9593bc610b370886a2b5f5acd3e1f58589"),
    entry!("31b",  "Q4_K_M", "7970aee3005211df5d90740a3e009a9b26be4df9", 19526590304, "ae935e91092f67df6254388c510cb89870353055912e452bb9e4923fd4f41071"),
    entry!("31b",  "Q3_K_M", "7970aee3005211df5d90740a3e009a9b26be4df9", 15863127904, "52918573c603e656f29d221f11ced1709a94df134d58dffa603806f77f947942"),
    entry!("31b",  "Q2_K",   "7970aee3005211df5d90740a3e009a9b26be4df9", 12609920864, "37d16404bc852dc8bf8ca95d6411e90b3bc0ff754b3c68474736ea59be3959ab"),
];

/// The files of the first revisions (2026-11-04), by id: (bytes, sha256). PFN replaced them on
/// 2026-11-06 with files that differ only in their chat template, which this app does not use
/// (it writes its own prompts), and they are no longer served. A file of that revision that was
/// downloaded is used as it is; an entry for it (from a catalog cached by an earlier run) is
/// outdated and ignored.
#[rustfmt::skip]
pub const SUPERSEDED: &[(&str, u64, &str)] = &[
    ("plamo-3-translate-0.8b-Q4_K_M", 573116288, "3dbb09295bfd3350231ba0dba92bc72cb14111ef1b034e37dad39d581bd86b9c"),
    ("plamo-3-translate-0.8b-Q4_0", 547532672, "2805c14de045d50f98e35e81aaf0fd23f6baa49aa8e06efcc11c46d44021bd05"),
    ("plamo-3-translate-0.8b-Q3_K_M", 486375296, "c6e6a9d0b05f3c0a7a4b70c6460a68dcf169d417984024b761c50e42e5713cdb"),
    ("plamo-3-translate-0.8b-Q2_K", 410128256, "30ae7b485e0f54980c4a5223d8415adbe04c382b36861687361b2b492929cc16"),
    ("plamo-3-translate-2b-Q4_K_M", 1807192480, "2c288ecc7ebfe60c3561726580da3583767f3c41ca3ef5c6825bf6c4cc060787"),
    ("plamo-3-translate-2b-Q4_0", 1698566560, "7a1839ed7f315b03176a7c7bb697629a95f8c3a61e669dfdf1cbeb6471551a66"),
    ("plamo-3-translate-2b-Q3_K_M", 1495876000, "a940db3afd6749bc74e550e324cccf1beb639cbf2dfe449f9a7f143e696fbec5"),
    ("plamo-3-translate-2b-Q2_K", 1225601440, "a2a02331f4922d2e283ac83c45cdbe2ab0faaf880e613078cde1299d7be78e2a"),
    ("plamo-3-translate-8b-Q4_K_M", 5280435168, "6b429549113ce0a61281ac72be195697241f89266f4247efec4893e4f580cf10"),
    ("plamo-3-translate-8b-Q3_K_M", 4310207456, "2413a3065f8666303d13b39161936786f2d8db9c4f1de4c2d2fcf80a72652fdc"),
    ("plamo-3-translate-8b-Q2_K", 3450735584, "610bd828dfe6f372437eb35b9be61fefeaa1aaa7861bce8446009f09f1545d3c"),
    ("plamo-3-translate-31b-Q4_K_M", 19526583392, "d1ed08149f888ef7a0da38e26bf394a13d0f079b592229c8778f5f1a91cf6457"),
    ("plamo-3-translate-31b-Q3_K_M", 15863120992, "f7b5ff4ce1f893acf029a4b00360bb59898399162576c1d1bbcd587b13c9553a"),
    ("plamo-3-translate-31b-Q2_K", 12609913952, "3565eb103a56cd7d8a3229f44a33b2bd601bcfa71018a4eed9f6648629ddaf03"),
];

/// Whether `bytes` is the size of the first revision of the file `id` (see [`SUPERSEDED`]).
fn is_superseded_size(id: &str, bytes: u64) -> bool {
    SUPERSEDED.iter().any(|&(i, b, _)| i == id && b == bytes)
}

/// Entries registered at run time. Each is leaked once, so `&'static` references stay valid.
static REGISTERED: RwLock<Vec<&'static CatalogEntry>> = RwLock::new(Vec::new());

/// Adds entries found at run time. An entry with the id of an existing one replaces it (a newer
/// revision); identical entries are ignored. Returns how many entries were added or replaced.
pub fn register(entries: impl IntoIterator<Item = CatalogEntry>) -> usize {
    let mut registered = REGISTERED.write().unwrap_or_else(|e| e.into_inner());
    let mut changed = 0;
    for entry in entries {
        if !SUPPORTED_FAMILIES.contains(&entry.family)
            || SUPERSEDED.iter().any(|&(_, _, sha)| sha == entry.sha256)
        {
            continue;
        }
        let current = registered
            .iter()
            .rev()
            .find(|e| e.id() == entry.id())
            .copied()
            .or_else(|| BUILTIN.iter().find(|e| e.id() == entry.id()));
        if current.is_some_and(|c| c.sha256 == entry.sha256) {
            continue;
        }
        registered.retain(|e| e.id() != entry.id());
        registered.push(Box::leak(Box::new(entry)));
        changed += 1;
    }
    changed
}

/// All known entries, newest revision of each file, sorted by family, size and quantization.
pub fn entries() -> Vec<&'static CatalogEntry> {
    let registered = REGISTERED.read().unwrap_or_else(|e| e.into_inner());
    let mut all: Vec<&'static CatalogEntry> = BUILTIN
        .iter()
        .filter(|b| !registered.iter().any(|r| r.id() == b.id()))
        .chain(registered.iter().copied())
        .collect();
    all.sort_by(|a, b| {
        a.family
            .cmp(b.family)
            .then(a.params_billion().total_cmp(&b.params_billion()))
            .then(quant_rank(a.quant).cmp(&quant_rank(b.quant)))
            .then(a.quant.cmp(b.quant))
    });
    all
}

/// Display order of quantizations: higher precision first, then the published 4-bit default.
fn quant_rank(quant: &str) -> usize {
    const ORDER: &[&str] = &[
        "BF16", "F16", "Q8_0", "Q6_K", "Q5_K_M", "Q5_K_S", "Q5_0", "Q4_K_M", "Q4_K_S", "Q4_0",
        "Q3_K_M", "Q3_K_S", "Q2_K",
    ];
    ORDER
        .iter()
        .position(|q| *q == quant)
        .unwrap_or(ORDER.len())
}

/// The entry used when nothing else is known about the machine.
pub fn default_entry() -> &'static CatalogEntry {
    find_entry("plamo-3-translate-2b-Q4_K_M").expect("default model is in the catalog")
}

pub fn find_entry(id: &str) -> Option<&'static CatalogEntry> {
    entries().into_iter().find(|e| e.id() == id)
}

/// Picks the recommended model for the given amount of memory available to the model.
///
/// The model file plus working memory should fit with some headroom, so this asks for
/// roughly 1.5x the file size. Models above 8B are never recommended automatically.
pub fn recommend(available_bytes: u64) -> &'static CatalogEntry {
    let candidates: Vec<_> = entries()
        .into_iter()
        .filter(|e| e.family == SUPPORTED_FAMILIES[0] && e.is_recommended_quant())
        .filter(|e| e.params_billion() <= 8.0)
        .collect();
    candidates
        .iter()
        .filter(|e| e.bytes.saturating_mul(3) / 2 <= available_bytes)
        .max_by(|a, b| a.params_billion().total_cmp(&b.params_billion()))
        .or_else(|| {
            candidates
                .iter()
                .min_by(|a, b| a.params_billion().total_cmp(&b.params_billion()))
        })
        .copied()
        .unwrap_or_else(default_entry)
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
        // Named after the expected digest too, so a partial download of an older revision is
        // never resumed against a newer one.
        let digest = entry.sha256.get(..12).unwrap_or(entry.sha256);
        self.dir.join(format!("{}.{digest}.part", entry.file_name))
    }

    /// Files only reach their final name after verification, so a final file of the right
    /// size counts as downloaded.
    pub fn state(&self, entry: &CatalogEntry) -> LocalState {
        if std::fs::metadata(self.path(entry))
            .is_ok_and(|m| m.len() == entry.bytes || is_superseded_size(entry.id(), m.len()))
        {
            return LocalState::Downloaded;
        }
        match std::fs::metadata(self.partial_path(entry)) {
            Ok(m) if m.len() > 0 => LocalState::Partial(m.len()),
            _ => LocalState::Missing,
        }
    }

    /// Partial downloads of `entry`'s file for other revisions (another digest in the name):
    /// they can never be resumed, and the files of older revisions are no longer served.
    pub fn stale_partials(&self, entry: &CatalogEntry) -> Vec<PathBuf> {
        let current = self.partial_path(entry);
        let prefix = format!("{}.", entry.file_name);
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        dir.flatten()
            .map(|e| e.path())
            .filter(|path| *path != current)
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_prefix(&prefix)?.strip_suffix(".part"))
                    .is_some_and(|digest| {
                        digest.len() == 12 && digest.bytes().all(|b| b.is_ascii_hexdigit())
                    })
            })
            .collect()
    }

    /// Removes the stale partial downloads of `entry` (see [`Self::stale_partials`]), returning
    /// the bytes freed.
    pub fn remove_stale_partials(&self, entry: &CatalogEntry) -> u64 {
        self.stale_partials(entry)
            .into_iter()
            .filter_map(|path| {
                let len = std::fs::metadata(&path).ok()?.len();
                std::fs::remove_file(&path).ok().map(|()| len)
            })
            .sum()
    }

    /// Deletes the model and any partial download, of this revision or an older one.
    pub fn delete(&self, entry: &CatalogEntry) -> io::Result<()> {
        for path in [self.path(entry), self.partial_path(entry)]
            .into_iter()
            .chain(self.stale_partials(entry))
        {
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
    fn builtin_catalog_is_consistent() {
        assert_eq!(BUILTIN.len(), 14);
        for e in BUILTIN {
            assert_eq!(
                e.file_name,
                format!("{}-{}-{}.gguf", e.family, e.size, e.quant)
            );
            assert_eq!(e.sha256.len(), 64);
            assert!(e.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert_eq!(e.revision.len(), 40);
            assert!(e.quality().is_some(), "{} has no quality data", e.id());
            assert!(e.reference_quality().is_some());
        }
        assert_eq!(default_entry().to_string(), "2B · Q4_K_M");
        assert_eq!(default_entry().params_billion(), 2.0);
    }

    #[test]
    fn url_points_to_pinned_revision() {
        let e = default_entry();
        assert_eq!(
            e.url(),
            "https://huggingface.co/pfnet/plamo-3-translate-2b-GGUF/resolve/\
             232fbd7916519edecddfdae0beae7e76fe70453f/plamo-3-translate-2b-Q4_K_M.gguf"
        );
    }

    #[test]
    fn entries_are_sorted_by_size_then_quantization() {
        let ids: Vec<_> = entries().iter().map(|e| e.id()).take(5).collect();
        assert_eq!(
            ids,
            [
                "plamo-3-translate-0.8b-Q4_K_M",
                "plamo-3-translate-0.8b-Q4_0",
                "plamo-3-translate-0.8b-Q3_K_M",
                "plamo-3-translate-0.8b-Q2_K",
                "plamo-3-translate-2b-Q4_K_M",
            ]
        );
    }

    #[test]
    fn registered_entries_add_and_supersede() {
        let base = *find_entry("plamo-3-translate-8b-Q4_K_M").unwrap();
        // A new quantization appears.
        let q8 = CatalogEntry {
            quant: "Q8_0",
            file_name: "plamo-3-translate-8b-Q8_0.gguf",
            sha256: "aa",
            ..base
        };
        // The same file at a newer revision.
        let newer = CatalogEntry {
            revision: "ffffffffffffffffffffffffffffffffffffffff",
            sha256: "bb",
            ..base
        };
        // An unsupported family is ignored.
        let other = CatalogEntry {
            family: "plamo-9-chat",
            file_name: "plamo-9-chat-8b-Q4_K_M.gguf",
            ..base
        };
        // Registering a built-in entry unchanged is a no-op.
        assert_eq!(register([base]), 0);
        assert_eq!(register([q8, newer, other]), 2);
        assert_eq!(register([q8]), 0);
        let found = find_entry("plamo-3-translate-8b-Q4_K_M").unwrap();
        assert_eq!(found.sha256, "bb");
        let order: Vec<_> = entries()
            .iter()
            .filter(|e| e.size == "8b")
            .map(|e| e.quant)
            .collect();
        assert_eq!(order, ["Q8_0", "Q4_K_M", "Q3_K_M", "Q2_K"]);
        assert!(find_entry("plamo-9-chat-8b-Q4_K_M").is_none());
    }

    #[test]
    fn recommends_by_available_memory() {
        const GB: u64 = 1 << 30;
        assert_eq!(recommend(16 * GB).size, "8b");
        assert_eq!(recommend(4 * GB).size, "2b");
        assert_eq!(recommend(GB).size, "0.8b");
        assert_eq!(recommend(0).size, "0.8b");
        // 31B is never recommended automatically.
        assert_eq!(recommend(128 * GB).size, "8b");
    }

    #[test]
    fn store_reports_local_state() {
        let dir = std::env::temp_dir().join(format!("plamo-store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = ModelStore::new(&dir);
        let e = &BUILTIN[0];
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

    #[test]
    fn stale_partials_are_removed() {
        let dir = std::env::temp_dir().join(format!("plamo-stale-parts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = ModelStore::new(&dir);
        let e = &BUILTIN[0];
        // A partial download of an older revision, the current one, and other files.
        let old = dir.join(format!("{}.0123456789ab.part", e.file_name));
        std::fs::write(&old, b"old").unwrap();
        std::fs::write(store.partial_path(e), b"current").unwrap();
        std::fs::write(dir.join(format!("{}.notes.part", e.file_name)), b"x").unwrap();
        std::fs::write(
            dir.join(format!("{}.0123456789ab.part", BUILTIN[1].file_name)),
            b"y",
        )
        .unwrap();
        assert_eq!(store.stale_partials(e), vec![old.clone()]);
        assert_eq!(store.remove_stale_partials(e), 3);
        assert!(!old.exists());
        assert_eq!(store.state(e), LocalState::Partial(7));
        // Deleting removes older revisions' parts too.
        std::fs::write(&old, b"old").unwrap();
        store.delete(e).unwrap();
        assert!(!old.exists() && !store.partial_path(e).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn first_revision_files_still_count() {
        // A catalog cached before 2026-11-06 lists the first revision: ignored.
        let current = *find_entry("plamo-3-translate-31b-Q2_K").unwrap();
        let &(_, bytes, sha256) = SUPERSEDED.iter().find(|s| s.0 == current.id()).unwrap();
        let first = CatalogEntry {
            revision: "d0df961b923db3ff9fc325f4410d6f09005afcd6",
            bytes,
            sha256,
            ..current
        };
        assert_eq!(register([first]), 0);
        assert_eq!(find_entry(current.id()).unwrap().sha256, current.sha256);

        // A downloaded file of the first revision is used as it is (a sparse file of its size).
        let dir = std::env::temp_dir().join(format!("plamo-first-revision-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = ModelStore::new(&dir);
        let e = find_entry("plamo-3-translate-0.8b-Q2_K").unwrap();
        let &(_, first_bytes, _) = SUPERSEDED.iter().find(|s| s.0 == e.id()).unwrap();
        assert_ne!(first_bytes, e.bytes);
        let file = std::fs::File::create(store.path(e)).unwrap();
        file.set_len(first_bytes).unwrap();
        assert_eq!(store.state(e), LocalState::Downloaded);
        file.set_len(first_bytes - 1).unwrap();
        assert_eq!(store.state(e), LocalState::Missing);
        drop(file);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_entry_has_a_first_revision() {
        for e in BUILTIN {
            assert!(
                SUPERSEDED.iter().any(|s| s.0 == e.id() && s.2 != e.sha256),
                "{}",
                e.id()
            );
        }
    }
}
