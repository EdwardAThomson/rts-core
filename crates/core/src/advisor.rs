//! Each faction's advisor as a setting pack describes it: who they are (a name, a title and a portrait), what they say
//! when a game starts and about a skirmish, and their help entry for each unit and building, in their own words. Both
//! engines show the advisor in the same places: beside a mission's briefing, and on the in-game advisor screen, where
//! the briefing, the objectives, the messages so far and the help entries can be read at any time. How the screen
//! looks, and the numbers an entry shows beside its words, are the engine's business; nothing here knows a unit.
//!
//! The pack's root `advisor.json` gives the advisor every faction shares at its top level, and each faction's own
//! under `factions`, which wins field by field (and entry by entry):
//!
//! ```json
//! {
//!   "name": "The advisor",
//!   "title": "Field advisor",
//!   "portrait": "ui/advisor.png",
//!   "greeting": "Words for the start of a game.",
//!   "skirmish": "What the advisor says about a skirmish, on the advisor screen.\n\nA blank line starts a paragraph.",
//!   "entries": { "power_plant": "The help entry for a unit or building, by its generic id." },
//!   "factions": {
//!     "faction_a": { "name": "Another advisor", "portrait": "ui/advisor_a.png", "entries": { "tank": "..." } }
//!   }
//! }
//! ```
//!
//! Every field may be left out. A portrait is a PNG file in the pack; without one the engines draw a plain frame with
//! the advisor's initials. `about` and `voice` are notes for people and tools, and are left alone.
//! Design: playbooks `plans/rts/ui.md`, "In-game advisor", and `plans/rts/maps-campaign.md`, "Briefings and the
//! advisor".

use std::collections::BTreeMap;

use crate::json::{self, Value};

/// Where a pack keeps its advisors, in its folder.
pub const FILE: &str = "advisor.json";

/// One advisor, as one faction sees them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Advisor {
    /// The advisor's name; empty when the pack gives none, and the engine's own word ("Advisor") shows.
    pub name: String,
    /// What they are, such as "Field advisor"; may be empty.
    pub title: String,
    /// The portrait's path in the pack, a PNG, if any.
    pub portrait: Option<String>,
    /// Words for the start of a game, if any.
    pub greeting: Option<String>,
    /// What the advisor says about a skirmish on the advisor screen, if anything; paragraphs split by a blank line.
    pub skirmish: Option<String>,
    /// Help entries by generic id.
    pub entries: BTreeMap<String, String>,
}

/// Every faction's advisor in a pack.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Advisors {
    /// The one every faction shares, before a faction's own fields.
    pub shared: Advisor,
    /// Each faction's own, by faction id, as written (fields left out are empty here).
    pub factions: BTreeMap<String, Advisor>,
}

const FIELDS: [&str; 6] = ["name", "title", "portrait", "greeting", "skirmish", "entries"];
const NOTES: [&str; 2] = ["about", "voice"];

impl Advisors {
    /// Read `advisor.json`. `factions` are the pack's faction ids and `ids` the generic ids it has entries for may
    /// name; anything wrong is reported, with its JSON path, and left out, so a slip in the file never stops a game.
    pub fn load(text: &str, factions: &[String], ids: &[String]) -> (Advisors, Vec<String>) {
        let mut warnings = Vec::new();
        let v = match json::parse(text) {
            Ok(v) if v.as_object().is_some() => v,
            Ok(_) => return (Advisors::default(), vec![format!("{FILE}: needs to be a JSON object")]),
            Err(e) => return (Advisors::default(), vec![format!("{FILE}: {e}")]),
        };
        let mut out = Advisors { shared: read(&v, "", ids, true, &mut warnings), ..Advisors::default() };
        if let Some(fs) = v.get("factions") {
            match fs.as_object() {
                Some(fs) => {
                    for (id, part) in fs {
                        if !factions.contains(id) {
                            warnings.push(format!("factions.{id}: no faction `{id}` in the pack"));
                            continue;
                        }
                        if part.as_object().is_none() {
                            warnings.push(format!("factions.{id}: needs to be an object"));
                            continue;
                        }
                        let own = read(part, &format!("factions.{id}."), ids, false, &mut warnings);
                        out.factions.insert(id.clone(), own);
                    }
                }
                None => warnings.push("factions: needs to be an object".to_string()),
            }
        }
        (out, warnings.into_iter().map(|w| format!("{FILE}: {w}")).collect())
    }

    /// The advisor `faction` sees: its own fields where it has them, else the shared ones; entries one by one.
    pub fn of(&self, faction: Option<&str>) -> Advisor {
        let mut a = self.shared.clone();
        let Some(own) = faction.and_then(|f| self.factions.get(f)) else { return a };
        if !own.name.is_empty() {
            a.name = own.name.clone();
        }
        if !own.title.is_empty() {
            a.title = own.title.clone();
        }
        for (mine, theirs) in
            [(&mut a.portrait, &own.portrait), (&mut a.greeting, &own.greeting), (&mut a.skirmish, &own.skirmish)]
        {
            if theirs.is_some() {
                mine.clone_from(theirs);
            }
        }
        a.entries.extend(own.entries.iter().map(|(k, v)| (k.clone(), v.clone())));
        a
    }
}

impl Advisor {
    /// The name to show: the advisor's, or `fallback` (the engine's word) when the pack gives none.
    pub fn name_or<'a>(&'a self, fallback: &'a str) -> &'a str {
        if self.name.trim().is_empty() { fallback } else { &self.name }
    }

    /// The initials of the name, at most two, upper case, for a portrait the pack doesn't draw.
    pub fn initials(&self, fallback: &str) -> String {
        self.name_or(fallback)
            .split_whitespace()
            .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
            .take(2)
            .flat_map(char::to_uppercase)
            .collect()
    }
}

/// `text` as paragraphs: split at blank lines, each trimmed, empty ones left out.
pub fn paragraphs(text: &str) -> Vec<String> {
    text.split("\n\n").map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// One advisor object: `at` is its JSON path's start, for warnings; `top` allows `factions` and the notes.
fn read(v: &Value, at: &str, ids: &[String], top: bool, warnings: &mut Vec<String>) -> Advisor {
    let mut a = Advisor::default();
    for (k, _) in v.as_object().unwrap_or_default() {
        let known = FIELDS.contains(&k.as_str()) || NOTES.contains(&k.as_str()) || (top && k == "factions");
        if !known {
            warnings.push(format!("{at}{k}: unknown field"));
        }
    }
    let mut text = |key: &str| -> Option<String> {
        let t = v.get(key)?;
        match t.as_str().map(str::trim) {
            Some(s) if !s.is_empty() => Some(s.to_string()),
            _ => {
                warnings.push(format!("{at}{key}: needs to be words"));
                None
            }
        }
    };
    a.name = text("name").unwrap_or_default();
    a.title = text("title").unwrap_or_default();
    a.greeting = text("greeting");
    a.skirmish = text("skirmish");
    a.portrait = text("portrait");
    if let Some(p) = &a.portrait {
        let safe = !p.starts_with('/') && !p.contains('\\') && p.split('/').all(|s| !s.is_empty() && s != "..");
        if !safe || !p.to_ascii_lowercase().ends_with(".png") {
            warnings.push(format!("{at}portrait: needs to be a PNG file's path inside the pack"));
            a.portrait = None;
        }
    }
    if let Some(entries) = v.get("entries") {
        match entries.as_object() {
            Some(entries) => {
                for (id, t) in entries {
                    if !ids.contains(id) {
                        warnings.push(format!("{at}entries.{id}: no unit or building `{id}` in the pack"));
                    } else if let Some(s) = t.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                        a.entries.insert(id.clone(), s.to_string());
                    } else {
                        warnings.push(format!("{at}entries.{id}: needs to be words"));
                    }
                }
            }
            None => warnings.push(format!("{at}entries: needs to be an object")),
        }
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_faction_wins_field_by_field_and_entry_by_entry() {
        let text = r#"{
            "about": "notes",
            "name": "Shared Name", "title": "Advisor", "skirmish": "One.\n\nTwo.",
            "entries": { "plant": "Makes power.", "tank": "Shoots." },
            "factions": { "a": { "name": "Own Name", "portrait": "ui/a.png", "entries": { "tank": "Our tank." } } }
        }"#;
        let (adv, warnings) = Advisors::load(text, &strings(&["a", "b"]), &strings(&["plant", "tank"]));
        assert!(warnings.is_empty(), "{warnings:?}");
        let a = adv.of(Some("a"));
        assert_eq!(a.name, "Own Name");
        assert_eq!(a.title, "Advisor");
        assert_eq!(a.portrait.as_deref(), Some("ui/a.png"));
        assert_eq!(a.entries["tank"], "Our tank.");
        assert_eq!(a.entries["plant"], "Makes power.");
        assert_eq!(paragraphs(a.skirmish.as_deref().unwrap()), ["One.", "Two."]);
        let b = adv.of(Some("b"));
        assert_eq!((b.name.as_str(), b.portrait.as_ref()), ("Shared Name", None));
        assert_eq!(b.entries["tank"], "Shoots.");
        assert_eq!(adv.of(None), adv.shared);
        assert_eq!(a.initials("Advisor"), "ON");
        assert_eq!(Advisor::default().initials("Advisor"), "A");
    }

    #[test]
    fn slips_are_reported_and_left_out() {
        let text = r#"{
            "nmae": "typo", "portrait": "../outside.png",
            "entries": { "ghost": "No such unit.", "tank": 3 },
            "factions": { "z": {}, "a": { "factions": {} } }
        }"#;
        let (adv, warnings) = Advisors::load(text, &strings(&["a"]), &strings(&["tank"]));
        assert_eq!(
            warnings,
            [
                "advisor.json: nmae: unknown field",
                "advisor.json: portrait: needs to be a PNG file's path inside the pack",
                "advisor.json: entries.ghost: no unit or building `ghost` in the pack",
                "advisor.json: entries.tank: needs to be words",
                "advisor.json: factions.z: no faction `z` in the pack",
                "advisor.json: factions.a.factions: unknown field",
            ]
        );
        assert_eq!(adv.shared.portrait, None);
        assert!(adv.shared.entries.is_empty());
        assert!(Advisors::load("[1]", &[], &[]).1[0].contains("JSON object"));
    }
}
