//! Campaigns as data: a territory map, the levels played across it, and the player's progress. Each level offers one
//! or more territories, each with the mission played to take it, so choosing where to attack chooses the mission
//! variant. Winning a mission takes its territory and opens the next level; losing one means playing it again.
//!
//! The territory map is text, like the Classic engine's maps: one character per cell, each territory a letter of its
//! own and `.` for none. Neighbours follow from the cells, so nothing has to list them. What a mission is, how it is
//! loaded and how the map is drawn are the engine's business; nothing here knows a unit or a tile of the game.
//!
//! A campaign file:
//!
//! ```json
//! {
//!   "id": "faction_a",
//!   "name": "Test campaign",
//!   "faction": "faction_a",
//!   "map": ["aab..", "aabcc", ".ddcc"],
//!   "territories": [
//!     { "id": "t01", "key": "a", "name": "Home plateau" },
//!     { "id": "t02", "key": "b", "name": "Ridge" }
//!   ],
//!   "home": "t01",
//!   "levels": [
//!     [{ "territory": "t01", "mission": "missions/m01.json" }],
//!     [{ "territory": "t02", "mission": "missions/m02.json" }, { "territory": "t03", "mission": "missions/m03.json" }]
//!   ]
//! }
//! ```
//!
//! Design: playbooks `plans/rts/maps-campaign.md`, "Campaign structure".

use crate::json::{self, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Campaign {
    pub id: String,
    /// The name the menus show; the id when the file gives none.
    pub name: String,
    /// The setting pack's faction the player plays, if the campaign names one.
    pub faction: Option<String>,
    pub width: usize,
    pub height: usize,
    /// Each cell's territory, by index into `territories`, in row order; `None` for no territory.
    pub cells: Vec<Option<usize>>,
    pub territories: Vec<Territory>,
    /// The territory the player holds from the start, if any.
    pub home: Option<usize>,
    pub levels: Vec<Vec<Choice>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Territory {
    pub id: String,
    /// Its character on the map.
    pub key: char,
    pub name: String,
}

/// A territory a level offers and the mission played to take it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// By index into `Campaign::territories`.
    pub territory: usize,
    /// The mission file, as a path in the setting pack.
    pub mission: String,
}

impl Campaign {
    /// Read a campaign file. Every problem is reported with its JSON path before the campaign is refused.
    pub fn load(text: &str) -> Result<Campaign, Vec<String>> {
        let v = json::parse(text).map_err(|e| vec![e.to_string()])?;
        let mut errors = Vec::new();
        if let Some(fields) = v.as_object() {
            for (k, _) in fields {
                if !["id", "name", "faction", "map", "territories", "home", "levels"].contains(&k.as_str()) {
                    errors.push(format!("{k}: unknown field"));
                }
            }
        }
        let mut text_of = |key: &str| match v.get(key) {
            None => None,
            Some(Value::Str(s)) if !s.is_empty() => Some(s.clone()),
            Some(_) => {
                errors.push(format!("{key}: must be a string"));
                None
            }
        };
        let (id, name, faction) = (text_of("id"), text_of("name"), text_of("faction"));
        if id.is_none() && v.get("id").is_none() {
            errors.push("id: missing".into());
        }

        let mut territories: Vec<Territory> = Vec::new();
        match v.get("territories").and_then(Value::as_array) {
            Some(list) if !list.is_empty() => {
                for (i, t) in list.iter().enumerate() {
                    let id = t.get("id").and_then(Value::as_str).filter(|s| !s.is_empty());
                    let key = t.get("key").and_then(Value::as_str).and_then(|k| {
                        let mut c = k.chars();
                        c.next().filter(|_| c.next().is_none())
                    });
                    let Some((id, key)) = id.zip(key.filter(|&k| k != '.' && !k.is_whitespace())) else {
                        errors.push(format!("territories[{i}]: needs an id and a key of one character, not ."));
                        continue;
                    };
                    if territories.iter().any(|o| o.id == id || o.key == key) {
                        errors.push(format!("territories[{i}]: id {id} or key {key} is used twice"));
                        continue;
                    }
                    let name = t.get("name").and_then(Value::as_str).unwrap_or(id).to_string();
                    territories.push(Territory { id: id.to_string(), key, name });
                }
            }
            _ => errors.push("territories: needs a list of at least one territory".into()),
        }

        let rows: Vec<&str> =
            v.get("map").and_then(Value::as_array).unwrap_or_default().iter().filter_map(Value::as_str).collect();
        let width = rows.first().map_or(0, |r| r.chars().count());
        if rows.is_empty() || width == 0 || rows.iter().any(|r| r.chars().count() != width) {
            errors.push("map: must be a list of rows of text, all the same length".into());
        }
        let mut cells = Vec::new();
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                match (c, territories.iter().position(|t| t.key == c)) {
                    ('.', _) => cells.push(None),
                    (_, Some(t)) => cells.push(Some(t)),
                    (_, None) => {
                        errors.push(format!("map[{y}]: {c} at column {x} is no territory's key"));
                        cells.push(None);
                    }
                }
            }
        }
        for t in &territories {
            if !cells.contains(&territories.iter().position(|o| o.id == t.id)) {
                errors.push(format!("territories: {} has no cell on the map", t.id));
            }
        }

        let find = |id: &str| territories.iter().position(|t| t.id == id);
        let home = match v.get("home") {
            None => None,
            Some(h) => match h.as_str().and_then(find) {
                Some(t) => Some(t),
                None => {
                    errors.push("home: must be a territory's id".into());
                    None
                }
            },
        };
        let mut levels = Vec::new();
        match v.get("levels").and_then(Value::as_array) {
            Some(list) if !list.is_empty() => {
                for (l, level) in list.iter().enumerate() {
                    let mut choices = Vec::new();
                    match level.as_array() {
                        Some(cs) if !cs.is_empty() => {
                            for (c, choice) in cs.iter().enumerate() {
                                let territory = choice.get("territory").and_then(Value::as_str).and_then(find);
                                let mission = choice.get("mission").and_then(Value::as_str).filter(|m| !m.is_empty());
                                match (territory, mission) {
                                    (Some(territory), Some(m)) => {
                                        if choices.iter().any(|o: &Choice| o.territory == territory) {
                                            errors.push(format!("levels[{l}][{c}]: the territory is offered twice"));
                                        }
                                        choices.push(Choice { territory, mission: m.to_string() });
                                    }
                                    _ => errors.push(format!(
                                        "levels[{l}][{c}]: needs a territory (a territory's id) and a mission (a path)"
                                    )),
                                }
                            }
                        }
                        _ => errors.push(format!("levels[{l}]: needs a list of at least one choice")),
                    }
                    levels.push(choices);
                }
            }
            _ => errors.push("levels: needs a list of at least one level".into()),
        }
        let id = id.unwrap_or_default();
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(Campaign {
            name: name.unwrap_or_else(|| id.clone()),
            id,
            faction,
            width,
            height: rows.len(),
            cells,
            territories,
            home,
            levels,
        })
    }

    /// The territory at cell (x, y), if any.
    pub fn at(&self, x: usize, y: usize) -> Option<usize> {
        if x < self.width && y < self.height { self.cells[y * self.width + x] } else { None }
    }

    /// Whether two territories share an edge on the map.
    pub fn neighbours(&self, a: usize, b: usize) -> bool {
        (0..self.height).any(|y| {
            (0..self.width).any(|x| {
                self.at(x, y) == Some(a) && (self.at(x + 1, y) == Some(b) || self.at(x, y + 1) == Some(b))
                    || self.at(x, y) == Some(b) && (self.at(x + 1, y) == Some(a) || self.at(x, y + 1) == Some(a))
            })
        })
    }

    /// The paths of the missions the campaign plays, each once, in level order.
    pub fn missions(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for c in self.levels.iter().flatten() {
            if !out.contains(&c.mission.as_str()) {
                out.push(&c.mission);
            }
        }
        out
    }
}

/// A mission won in a campaign.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Won {
    /// The territory taken, by id.
    pub territory: String,
    /// The mission played, as its path in the pack.
    pub mission: String,
    /// Game ticks it took.
    pub ticks: u32,
}

/// How far the player has got in one campaign: a mission won for each level done, in order. Kept apart from saved
/// games, as a small text file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub won: Vec<Won>,
}

/// The first line of every progress file, with the format's version.
const MAGIC: &str = "campaign-progress 1";

impl Progress {
    /// The level to play next, from 0; `campaign.levels.len()` once the campaign is done.
    pub fn level(&self) -> usize {
        self.won.len()
    }

    pub fn done(&self, campaign: &Campaign) -> bool {
        self.level() >= campaign.levels.len()
    }

    /// The choices the next level offers; none once the campaign is done.
    pub fn choices<'c>(&self, campaign: &'c Campaign) -> &'c [Choice] {
        campaign.levels.get(self.level()).map_or(&[], Vec::as_slice)
    }

    /// The territories the player holds: home, then each taken, by index into `campaign.territories`.
    pub fn held(&self, campaign: &Campaign) -> Vec<usize> {
        let taken = self.won.iter().filter_map(|w| campaign.territories.iter().position(|t| t.id == w.territory));
        campaign.home.into_iter().chain(taken).collect()
    }

    /// Record a win of the next level's `choice` (by index into `choices`), in `ticks`. A choice that isn't offered
    /// changes nothing and returns false.
    pub fn win(&mut self, campaign: &Campaign, choice: usize, ticks: u32) -> bool {
        let Some(c) = self.choices(campaign).get(choice) else { return false };
        let territory = campaign.territories[c.territory].id.clone();
        self.won.push(Won { territory, mission: c.mission.clone(), ticks });
        true
    }

    /// The progress as its text file.
    pub fn to_text(&self) -> String {
        let mut out = format!("{MAGIC}\n");
        for w in &self.won {
            out.push_str(&format!("won {} {} {}\n", w.territory, w.ticks, w.mission));
        }
        out
    }

    /// Read a progress file, checking it against `campaign`: a level won in a territory it doesn't offer means the
    /// campaign has changed, and the progress is refused.
    pub fn parse(text: &str, campaign: &Campaign) -> Result<Progress, String> {
        let mut lines = text.lines();
        if lines.next() != Some(MAGIC) {
            return Err("not a campaign progress file".into());
        }
        let mut p = Progress::default();
        for line in lines.filter(|l| !l.trim().is_empty()) {
            let mut parts = line.splitn(4, ' ');
            let (Some("won"), Some(territory), Some(ticks), Some(mission)) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                return Err(format!("can't read the line {line:?}"));
            };
            let ticks = ticks.parse().map_err(|_| format!("can't read the line {line:?}"))?;
            let offered = p.choices(campaign).iter().any(|c| campaign.territories[c.territory].id == territory);
            if !offered {
                return Err(format!("level {} doesn't offer {territory}", p.level() + 1));
            }
            p.won.push(Won { territory: territory.to_string(), mission: mission.to_string(), ticks });
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"{
      "id": "test", "name": "Test campaign", "faction": "faction_a",
      "map": ["aab..",
              "aabcc",
              ".ddcc"],
      "territories": [
        { "id": "t01", "key": "a", "name": "Home" },
        { "id": "t02", "key": "b", "name": "Ridge" },
        { "id": "t03", "key": "c" },
        { "id": "t04", "key": "d", "name": "Flats" }
      ],
      "home": "t01",
      "levels": [
        [{ "territory": "t02", "mission": "missions/m01.json" }],
        [{ "territory": "t03", "mission": "missions/m02.json" }, { "territory": "t04", "mission": "missions/m03.json" }]
      ]
    }"#;

    #[test]
    fn a_campaign_reads_its_map_and_levels() {
        let c = Campaign::load(FILE).unwrap();
        assert_eq!(
            (c.id.as_str(), c.name.as_str(), c.faction.as_deref()),
            ("test", "Test campaign", Some("faction_a"))
        );
        assert_eq!((c.width, c.height), (5, 3));
        assert_eq!((c.at(0, 0), c.at(2, 0), c.at(4, 0), c.at(9, 9)), (Some(0), Some(1), None, None));
        assert_eq!(c.territories[2].name, "t03", "the id when no name");
        assert_eq!(c.home, Some(0));
        assert!(c.neighbours(0, 1) && c.neighbours(1, 2) && c.neighbours(3, 0) && c.neighbours(2, 3));
        assert!(!c.neighbours(0, 2), "home and t03 don't touch");
        assert_eq!(c.missions(), ["missions/m01.json", "missions/m02.json", "missions/m03.json"]);
    }

    #[test]
    fn progress_takes_territories_level_by_level_and_survives_its_file() {
        let c = Campaign::load(FILE).unwrap();
        let mut p = Progress::default();
        assert_eq!(p.choices(&c).len(), 1);
        assert_eq!(p.held(&c), [0]);
        assert!(!p.win(&c, 1, 10), "level 1 offers one choice");
        assert!(p.win(&c, 0, 6630));
        assert_eq!(p.choices(&c).iter().map(|ch| ch.territory).collect::<Vec<_>>(), [2, 3]);
        assert!(p.win(&c, 1, 9000));
        assert!(p.done(&c) && p.choices(&c).is_empty());
        assert_eq!(p.held(&c), [0, 1, 3]);
        let text = p.to_text();
        assert_eq!(text, "campaign-progress 1\nwon t02 6630 missions/m01.json\nwon t04 9000 missions/m03.json\n");
        assert_eq!(Progress::parse(&text, &c).unwrap(), p);
        assert_eq!(Progress::parse("campaign-progress 1\n", &c).unwrap(), Progress::default());
        assert!(
            Progress::parse("campaign-progress 1\nwon t03 5 missions/m02.json\n", &c).unwrap_err().contains("level 1")
        );
        assert!(Progress::parse("hello", &c).is_err());
    }

    #[test]
    fn a_faulty_campaign_lists_every_problem() {
        let text = r#"{
          "colour": "red", "map": ["ab", "a"],
          "territories": [{ "id": "t01", "key": "a" }, { "id": "t01", "key": "z" }, { "id": "t03", "key": ".." }],
          "home": "t09",
          "levels": [[], [{ "territory": "t01" }]]
        }"#;
        let errors = Campaign::load(text).unwrap_err();
        for want in [
            "colour: unknown field",
            "id: missing",
            "territories[1]: id t01 or key z is used twice",
            "territories[2]: needs an id and a key of one character, not .",
            "map: must be a list of rows of text, all the same length",
            "map[0]: b at column 1 is no territory's key",
            "home: must be a territory's id",
            "levels[0]: needs a list of at least one choice",
            "levels[1][0]: needs a territory (a territory's id) and a mission (a path)",
        ] {
            assert!(errors.iter().any(|e| e == want), "{want:?} in {errors:#?}");
        }
    }
}
