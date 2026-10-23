//! Setting packs as players get them: a pack folder holding `pack.json`, which says what the pack is, and a folder
//! for each engine it plays in, so one setting can ship for both engines in one folder:
//!
//! ```text
//! packs/<pack>/
//!   pack.json     id, title, version, tier, licence, which engines
//!   classic/      the Classic engine's pack (setting.json, names.json, art/, ...)
//!   3d/           the 3D engine's pack (setting.json, units.json, models/, ...)
//!   shared/       files both engines read (music, voices, logo)
//! ```
//!
//! `pack.json`:
//!
//! ```json
//! {
//!   "id": "pigeons-vs-magpies",
//!   "title": "Pigeons Vs Magpies",
//!   "version": "0.1.0",
//!   "tier": "free",
//!   "licence": "CC0-1.0",
//!   "engines": ["classic", "3d"],
//!   "description": "An urban park turf war.",
//!   "credits": "From the pvm concept."
//! }
//! ```
//!
//! Each engine looks for packs in a few folders ([`search`]): `packs/` beside the program, the shared folder in the
//! user's settings folder ([`user_folder`]), and whatever else it adds (the repository's own `packs/` while
//! developing). A folder in one of those is a pack when it holds `pack.json`, or a pack repository when it holds a
//! `packs/` folder of them, so a cloned pack repository can be dropped in whole. The browser can't list folders, so a
//! page offers its packs in `packs.json` instead ([`index`]).
//!
//! Free or premium is a field, not a place: the same loader reads both, and selling a pack is the store's business.
//! Design: playbooks `plans/rts/settings.md`, "Where packs live".

use std::path::{Path, PathBuf};

use crate::json::{self, Value};

/// A pack's own description, from its `pack.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// Lower case letters, digits, `-` and `_`; what saves, replays and `--setting`/`--pack` name it by.
    pub id: String,
    /// The name the pack list shows; the id when the file gives none.
    pub title: String,
    pub version: String,
    pub tier: Tier,
    /// The licence the pack is shared under, as given (an SPDX id such as `CC0-1.0`, or words); empty if none.
    pub licence: String,
    /// The engines it has a folder for, such as `classic` and `3d`, in file order.
    pub engines: Vec<String>,
    pub description: String,
    pub credits: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Free,
    Premium,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Free => "free",
            Tier::Premium => "premium",
        }
    }
}

/// The name of a pack's description file.
pub const MANIFEST: &str = "pack.json";
/// The name of the browser's list of packs, at the root the page serves.
pub const INDEX: &str = "packs.json";
/// The name of the folder searched for packs.
pub const FOLDER: &str = "packs";

impl Manifest {
    /// Read a `pack.json`, with every problem in the list.
    pub fn parse(text: &str) -> Result<Manifest, Vec<String>> {
        let doc = json::parse(text).map_err(|e| vec![format!("{MANIFEST}: {e}")])?;
        if doc.as_object().is_none() {
            return Err(vec![format!("{MANIFEST}: not an object")]);
        }
        let mut errors = Vec::new();
        let text = |key: &str| doc.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let id = text("id");
        if !valid_id(&id) {
            errors.push(format!("{MANIFEST}: id {id:?} must be lower case letters, digits, - and _"));
        }
        let version = text("version");
        if version.is_empty() {
            errors.push(format!("{MANIFEST}: no version"));
        }
        let tier = match text("tier").as_str() {
            "free" => Tier::Free,
            "premium" => Tier::Premium,
            other => {
                errors.push(format!("{MANIFEST}: tier {other:?} must be \"free\" or \"premium\""));
                Tier::Free
            }
        };
        let engines: Vec<String> = match doc.get("engines").and_then(Value::as_array) {
            Some(list) => list.iter().filter_map(Value::as_str).map(String::from).collect(),
            None => Vec::new(),
        };
        if engines.is_empty() {
            errors.push(format!("{MANIFEST}: engines must list at least one engine"));
        }
        if let Some(bad) = engines.iter().find(|e| !valid_id(e)) {
            errors.push(format!("{MANIFEST}: engine {bad:?} must be lower case letters, digits, - and _"));
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let title = text("title");
        Ok(Manifest {
            title: if title.is_empty() { id.clone() } else { title },
            id,
            version,
            tier,
            licence: text("licence"),
            engines,
            description: text("description"),
            credits: text("credits"),
        })
    }

    pub fn plays_in(&self, engine: &str) -> bool {
        self.engines.iter().any(|e| e == engine)
    }
}

/// Whether `id` is a usable pack or engine id: not empty, lower case ASCII letters, digits, `-` and `_` only.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// A pack found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub manifest: Manifest,
    /// The pack folder, holding `pack.json`.
    pub dir: PathBuf,
}

impl Found {
    /// The folder holding the pack for `engine`, such as `<dir>/classic`.
    pub fn engine_dir(&self, engine: &str) -> PathBuf {
        self.dir.join(engine)
    }
}

/// What a search found: the packs that play in the engine, in the order of the folders searched and then by folder
/// name, and a line for each pack folder left out (a broken `pack.json`, no folder for the engine, an id already
/// taken by an earlier pack). A folder searched that doesn't exist is not a problem.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Search {
    pub packs: Vec<Found>,
    pub problems: Vec<String>,
}

/// Look for `engine`'s packs in `folders`, each a folder of packs and pack repositories (see the module notes).
pub fn search(folders: &[PathBuf], engine: &str) -> Search {
    let mut out = Search::default();
    for folder in folders {
        for dir in subfolders(folder) {
            if dir.join(MANIFEST).is_file() {
                consider(&dir, engine, &mut out);
            } else if dir.join(FOLDER).is_dir() {
                for inner in subfolders(&dir.join(FOLDER)) {
                    if inner.join(MANIFEST).is_file() {
                        consider(&inner, engine, &mut out);
                    }
                }
            }
        }
    }
    out
}

/// The pack in `found` whose id or folder name is `name`.
pub fn find<'a>(found: &'a [Found], name: &str) -> Option<&'a Found> {
    found
        .iter()
        .find(|f| f.manifest.id == name)
        .or_else(|| found.iter().find(|f| f.dir.file_name().is_some_and(|n| n == name)))
}

fn consider(dir: &Path, engine: &str, out: &mut Search) {
    let manifest = match std::fs::read_to_string(dir.join(MANIFEST))
        .map_err(|e| vec![e.to_string()])
        .and_then(|t| Manifest::parse(&t))
    {
        Ok(m) => m,
        Err(errors) => return out.problems.push(format!("{}: {}", dir.display(), errors.join("; "))),
    };
    if !manifest.plays_in(engine) {
        return;
    }
    if !dir.join(engine).is_dir() {
        return out.problems.push(format!("{}: lists {engine} but has no {engine}/ folder", dir.display()));
    }
    if let Some(first) = out.packs.iter().find(|f| f.manifest.id == manifest.id) {
        return out.problems.push(format!("{}: id {} is already {}", dir.display(), manifest.id, first.dir.display()));
    }
    out.packs.push(Found { manifest, dir: dir.to_path_buf() });
}

/// The folders inside `dir`, by name; none when it can't be read.
fn subfolders(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    found.sort();
    found
}

/// The `packs/` folder beside the running program, where a game ships its bundled packs.
pub fn beside_program() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join(FOLDER))
}

/// The user's own packs folder, shared by both engines so a pack dropped in once plays in each:
/// `%APPDATA%\rts-packs` on Windows, else `$XDG_CONFIG_HOME/rts-packs`, else `~/.config/rts-packs`.
pub fn user_folder() -> Option<PathBuf> {
    user_folder_from(cfg!(windows), |v| std::env::var_os(v).filter(|s| !s.is_empty()).map(PathBuf::from))
}

fn user_folder_from(windows: bool, var: impl Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let unix = || var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")));
    let base = if windows { var("APPDATA").or_else(unix) } else { unix().or_else(|| var("APPDATA")) };
    Some(base?.join("rts-packs"))
}

/// The pack folders a page's `packs.json` offers, each a path from the page's root:
/// `{ "packs": ["packs/pigeons-vs-magpies"] }`.
pub fn index(text: &str) -> Result<Vec<String>, String> {
    let doc = json::parse(text).map_err(|e| format!("{INDEX}: {e}"))?;
    let list = doc.get("packs").and_then(Value::as_array).ok_or_else(|| format!("{INDEX}: no packs list"))?;
    list.iter()
        .map(|v| {
            v.as_str()
                .map(|s| s.trim_end_matches('/').to_string())
                .ok_or_else(|| format!("{INDEX}: a pack that isn't a path"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PVM: &str = r#"{ "id": "pigeons-vs-magpies", "title": "Pigeons Vs Magpies", "version": "0.1.0",
        "tier": "free", "licence": "CC0-1.0", "engines": ["classic", "3d"] }"#;

    #[test]
    fn a_manifest_reads_and_defaults_its_title() {
        let m = Manifest::parse(PVM).unwrap();
        assert_eq!((m.id.as_str(), m.title.as_str(), m.tier), ("pigeons-vs-magpies", "Pigeons Vs Magpies", Tier::Free));
        assert!(m.plays_in("classic") && m.plays_in("3d") && !m.plays_in("other"));
        let bare = Manifest::parse(r#"{ "id": "x", "version": "1", "tier": "premium", "engines": ["3d"] }"#).unwrap();
        assert_eq!((bare.title.as_str(), bare.tier, bare.licence.as_str()), ("x", Tier::Premium, ""));
    }

    #[test]
    fn a_bad_manifest_lists_every_problem() {
        let errors = Manifest::parse(r#"{ "id": "Bad Id", "tier": "paid", "engines": [] }"#).unwrap_err();
        assert_eq!(errors.len(), 4, "{errors:?}");
        assert!(Manifest::parse("[]").is_err());
    }

    #[test]
    fn search_finds_packs_and_pack_repositories_and_reports_the_rest() {
        let root = std::env::temp_dir().join(format!("rts-core-packs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pack = |dir: &Path, id: &str, engines: &str| {
            std::fs::create_dir_all(dir).unwrap();
            let text = format!(r#"{{ "id": "{id}", "version": "1", "tier": "free", "engines": [{engines}] }}"#);
            std::fs::write(dir.join(MANIFEST), text).unwrap();
        };
        let a = root.join("one");
        pack(&a.join("alpha"), "alpha", r#""classic""#);
        std::fs::create_dir_all(a.join("alpha/classic")).unwrap();
        // A cloned pack repository: its packs are one level down.
        pack(&a.join("repo/packs/beta"), "beta", r#""classic", "3d""#);
        std::fs::create_dir_all(a.join("repo/packs/beta/classic")).unwrap();
        // Lists classic but has no folder for it; plays only in 3d; a broken file.
        pack(&a.join("gamma"), "gamma", r#""classic""#);
        pack(&a.join("delta"), "delta", r#""3d""#);
        std::fs::create_dir_all(a.join("eps")).unwrap();
        std::fs::write(a.join("eps").join(MANIFEST), "{").unwrap();
        // The same id again in a later folder.
        let b = root.join("two");
        pack(&b.join("alpha"), "alpha", r#""classic""#);
        std::fs::create_dir_all(b.join("alpha/classic")).unwrap();

        let found = search(&[a.clone(), root.join("missing"), b], "classic");
        let ids: Vec<&str> = found.packs.iter().map(|f| f.manifest.id.as_str()).collect();
        assert_eq!(ids, ["alpha", "beta"]);
        assert_eq!(found.problems.len(), 3, "{:?}", found.problems);
        assert_eq!(find(&found.packs, "beta").unwrap().engine_dir("classic"), a.join("repo/packs/beta/classic"));
        assert!(find(&found.packs, "delta").is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_user_folder_follows_the_settings_folder_rules() {
        let env =
            |pairs: &'static [(&str, &str)]| move |k: &str| pairs.iter().find(|p| p.0 == k).map(|p| PathBuf::from(p.1));
        assert_eq!(
            user_folder_from(false, env(&[("HOME", "/home/p")])),
            Some(PathBuf::from("/home/p/.config/rts-packs"))
        );
        assert_eq!(
            user_folder_from(true, env(&[("APPDATA", "C:/A"), ("HOME", "C:/U")])),
            Some(PathBuf::from("C:/A/rts-packs"))
        );
        assert_eq!(user_folder_from(false, env(&[])), None);
    }

    #[test]
    fn the_browser_index_lists_pack_paths() {
        assert_eq!(index(r#"{ "packs": ["packs/a/", "packs/b"] }"#).unwrap(), ["packs/a", "packs/b"]);
        assert!(index("{}").is_err());
    }
}
