//! Missions as data: a list of triggers, each a condition and the actions to take when it holds. The engine checks
//! them once a game second, in file order, and the state they leave (which triggers have fired, each objective's
//! state, who has won or lost) lives in the game state, so it is saved, hashed and replayed with everything else.
//!
//! The parts every engine shares live here: combining conditions (`all`, `any`, `not`), time, other triggers and
//! objectives, and the actions on objectives, winning, losing and advisor lines. Each engine adds its own conditions
//! and actions (resource delivered, units in an area, reinforcements) through the `C` and `A` type parameters and
//! the two parse callbacks; nothing here knows what a unit or a tile is.
//!
//! Declarative triggers rather than a scripting language, so missions stay deterministic, easy to check and easy to
//! write by hand (playbooks `plans/rts/maps-campaign.md`, "Mission scripting").

use crate::hash::{Canon, CanonHasher};
use crate::json::Value;

/// A mission's triggers, as loaded. Never changes during a game; `MissionState` holds what does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mission<C, A> {
    pub triggers: Vec<Trigger<C, A>>,
    /// Each objective's id, in the order objectives are first named; `Cond::Objective` and the objective actions
    /// refer to them by index into this list.
    pub objectives: Vec<String>,
    /// Ticks between checks: one game second.
    pub every: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trigger<C, A> {
    pub id: String,
    pub when: Cond<C>,
    pub actions: Vec<Action<A>>,
    /// Fire at every check where the condition holds, not just the first.
    pub repeat: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cond<C> {
    All(Vec<Cond<C>>),
    Any(Vec<Cond<C>>),
    Not(Box<Cond<C>>),
    /// At or after this tick.
    AtTime(u32),
    /// Every `ticks` ticks from `start`.
    Every {
        ticks: u32,
        start: u32,
    },
    /// The trigger at this index has fired at least once.
    Fired(usize),
    /// The objective at this index is in this state.
    Objective(usize, ObjectiveState),
    /// One of the engine's own conditions.
    Engine(C),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action<A> {
    /// Show the objective at this index, with the text id the setting pack spells out.
    SetObjective {
        objective: usize,
        text: String,
        primary: bool,
    },
    Complete(usize),
    Fail(usize),
    /// This player (0-based) wins.
    Win(u32),
    /// This player (0-based) loses.
    Lose(u32),
    /// An advisor line, by id: the engine reports it and the front end shows and speaks it.
    Say(String),
    /// One of the engine's own actions.
    Engine(A),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectiveState {
    /// Not yet set.
    Hidden,
    Open,
    Done,
    Failed,
}

impl ObjectiveState {
    pub fn id(self) -> &'static str {
        match self {
            ObjectiveState::Hidden => "hidden",
            ObjectiveState::Open => "open",
            ObjectiveState::Done => "done",
            ObjectiveState::Failed => "failed",
        }
    }

    fn from_id(id: &str) -> Option<ObjectiveState> {
        [ObjectiveState::Hidden, ObjectiveState::Open, ObjectiveState::Done, ObjectiveState::Failed]
            .into_iter()
            .find(|s| s.id() == id)
    }
}

/// One player's end, in the order they were decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub player: u32,
    pub won: bool,
}

/// What a mission has done so far: part of the game state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionState {
    /// How many times each trigger has fired, in trigger order.
    pub fired: Vec<u32>,
    /// Each objective's state, in objective order.
    pub objectives: Vec<ObjectiveState>,
    /// Each objective's text id and whether it is primary, once set.
    pub shown: Vec<Option<(String, bool)>>,
    pub outcomes: Vec<Outcome>,
}

impl MissionState {
    pub fn new<C, A>(mission: &Mission<C, A>) -> MissionState {
        MissionState {
            fired: vec![0; mission.triggers.len()],
            objectives: vec![ObjectiveState::Hidden; mission.objectives.len()],
            shown: vec![None; mission.objectives.len()],
            outcomes: Vec::new(),
        }
    }

    /// Whether this player has won (`Some(true)`), lost (`Some(false)`) or neither yet.
    pub fn outcome(&self, player: u32) -> Option<bool> {
        self.outcomes.iter().find(|o| o.player == player).map(|o| o.won)
    }
}

impl Canon for ObjectiveState {
    fn canon(&self, w: &mut CanonHasher) {
        w.string(self.id());
    }
}

impl Canon for Outcome {
    fn canon(&self, w: &mut CanonHasher) {
        w.object().field("player", &self.player).field("won", &self.won).end();
    }
}

impl Canon for MissionState {
    fn canon(&self, w: &mut CanonHasher) {
        // `shown` follows from the triggers that fired, so it isn't written.
        w.object()
            .field("fired", &self.fired)
            .field("objectives", &self.objectives)
            .field("outcomes", &self.outcomes)
            .end();
    }
}

impl<C, A> Mission<C, A> {
    /// Check every trigger if `tick` is a check tick, in file order, and fire those whose condition holds. A trigger
    /// sees what the triggers before it did in the same check. The shared actions are applied to `state` here; every
    /// action fired is returned in order, so the engine can apply its own and report the rest. `test` answers the
    /// engine's own conditions.
    pub fn check<'m>(
        &'m self,
        state: &mut MissionState,
        tick: u32,
        mut test: impl FnMut(&C) -> bool,
    ) -> Vec<&'m Action<A>> {
        let mut fired = Vec::new();
        if self.every == 0 || !tick.is_multiple_of(self.every) {
            return fired;
        }
        for (i, t) in self.triggers.iter().enumerate() {
            if (state.fired[i] > 0 && !t.repeat) || !holds(&t.when, state, tick, &mut test) {
                continue;
            }
            state.fired[i] += 1;
            for a in &t.actions {
                apply(a, state);
                fired.push(a);
            }
        }
        fired
    }
}

fn holds<C>(c: &Cond<C>, state: &MissionState, tick: u32, test: &mut impl FnMut(&C) -> bool) -> bool {
    match c {
        Cond::All(list) => list.iter().all(|c| holds(c, state, tick, test)),
        Cond::Any(list) => list.iter().any(|c| holds(c, state, tick, test)),
        Cond::Not(c) => !holds(c, state, tick, test),
        Cond::AtTime(t) => tick >= *t,
        Cond::Every { ticks, start } => tick >= *start && (tick - start).is_multiple_of((*ticks).max(1)),
        Cond::Fired(i) => state.fired[*i] > 0,
        Cond::Objective(i, s) => state.objectives[*i] == *s,
        Cond::Engine(c) => test(c),
    }
}

fn apply<A>(a: &Action<A>, state: &mut MissionState) {
    let mut end = |player: u32, won: bool| {
        if state.outcome(player).is_none() {
            state.outcomes.push(Outcome { player, won });
        }
    };
    match a {
        Action::SetObjective { objective, text, primary } => {
            state.objectives[*objective] = ObjectiveState::Open;
            state.shown[*objective] = Some((text.clone(), *primary));
        }
        Action::Complete(i) => state.objectives[*i] = ObjectiveState::Done,
        Action::Fail(i) => state.objectives[*i] = ObjectiveState::Failed,
        Action::Win(p) => end(*p, true),
        Action::Lose(p) => end(*p, false),
        Action::Say(_) | Action::Engine(_) => {}
    }
}

/// Reads the parts of a mission file every engine shares, handing the rest to the engine.
pub struct Reader<'a, C, A> {
    /// Engine ticks in a game second; times in the file are in seconds.
    pub ticks_per_second: u32,
    /// How many players the mission has, when known, so `win` and `lose` can refuse others.
    pub players: Option<u32>,
    /// The engine's own condition: its key and value. `None` when the key isn't one of the engine's.
    #[allow(clippy::type_complexity)]
    pub cond: &'a dyn Fn(&str, &Value) -> Option<Result<C, String>>,
    /// The engine's own action, likewise.
    #[allow(clippy::type_complexity)]
    pub action: &'a dyn Fn(&str, &Value) -> Option<Result<A, String>>,
}

/// A player number as a mission file writes it, 1-based, as the 0-based player index.
pub fn player(v: &Value) -> Result<u32, String> {
    match v.as_int() {
        Some(n) if (1..=i64::from(u32::MAX)).contains(&n) => Ok(n as u32 - 1),
        _ => Err(format!("a player must be a number from 1, not {}", v.kind())),
    }
}

/// Seconds as ticks.
pub fn seconds(v: &Value, ticks_per_second: u32) -> Result<u32, String> {
    match v.as_int() {
        Some(n) if (0..=i64::from(u32::MAX / ticks_per_second.max(1))).contains(&n) => Ok(n as u32 * ticks_per_second),
        _ => Err(format!("a time must be a whole number of seconds, not {}", v.kind())),
    }
}

/// A one-key object's key and value, which is how every condition and action is written:
/// `{ "at_time": 240 }`, `{ "win": 1 }`.
fn single(v: &Value) -> Result<(&str, &Value), String> {
    match v.as_object() {
        Some([(k, v)]) => Ok((k, v)),
        Some(fields) => Err(format!("must have exactly one key, not {}", fields.len())),
        None => Err(format!("must be an object, not {}", v.kind())),
    }
}

impl<C, A> Reader<'_, C, A> {
    /// A player number from the file, which must be one of the mission's players when their number is known.
    pub fn player(&self, v: &Value) -> Result<u32, String> {
        let p = player(v)?;
        match self.players {
            Some(n) if p >= n => Err(format!("player {} isn't in the mission's players", p + 1)),
            _ => Ok(p),
        }
    }

    /// Read a mission's `triggers` list. Every problem is reported, each with its JSON path
    /// (`triggers[2].when.all[0]: ...`), before the mission is refused.
    pub fn triggers(&self, list: &Value) -> Result<Mission<C, A>, Vec<String>> {
        let mut errors = Vec::new();
        let Some(items) = list.as_array() else {
            return Err(vec![format!("triggers: must be a list, not {}", list.kind())]);
        };
        // Trigger ids first, so a `fired` condition may name a later trigger.
        let mut ids: Vec<String> = Vec::new();
        for (i, t) in items.iter().enumerate() {
            match t.get("id").and_then(Value::as_str) {
                // Kept in the list anyway, so the triggers and their ids still line up.
                Some(id) => {
                    if ids.iter().any(|x| x == id) {
                        errors.push(format!("triggers[{i}].id: {id} is used twice"));
                    }
                    ids.push(id.to_string());
                }
                None => {
                    errors.push(format!("triggers[{i}].id: missing"));
                    ids.push(String::new());
                }
            }
        }
        // Objectives are named by `set_objective`; every other mention must match one.
        let mut objectives = Vec::new();
        for t in items {
            for a in t.get("do").and_then(Value::as_array).unwrap_or_default() {
                if let Ok(("set_objective", v)) = single(a)
                    && let Some(id) = v.get("id").and_then(Value::as_str)
                    && !objectives.iter().any(|o| o == id)
                {
                    objectives.push(id.to_string());
                }
            }
        }
        let names = Names { triggers: &ids, objectives: &objectives };
        let mut triggers = Vec::new();
        for (i, t) in items.iter().enumerate() {
            let path = format!("triggers[{i}]");
            if let Some(fields) = t.as_object() {
                for (k, _) in fields {
                    if !["id", "when", "do", "repeat"].contains(&k.as_str()) {
                        errors.push(format!("{path}.{k}: unknown field"));
                    }
                }
            }
            let when = match t.get("when") {
                Some(w) => self.cond(w, &format!("{path}.when"), &names, &mut errors),
                None => {
                    errors.push(format!("{path}.when: missing"));
                    None
                }
            };
            let mut actions = Vec::new();
            match t.get("do").map(|d| (d, d.as_array())) {
                Some((_, Some(list))) => {
                    for (j, a) in list.iter().enumerate() {
                        actions.extend(self.action(a, &format!("{path}.do[{j}]"), &names, &mut errors));
                    }
                }
                Some((d, None)) => errors.push(format!("{path}.do: must be a list, not {}", d.kind())),
                None => errors.push(format!("{path}.do: missing")),
            }
            let repeat = match t.get("repeat") {
                None => false,
                Some(r) => r.as_bool().unwrap_or_else(|| {
                    errors.push(format!("{path}.repeat: must be true or false"));
                    false
                }),
            };
            if let Some(when) = when {
                triggers.push(Trigger { id: ids[i].clone(), when, actions, repeat });
            }
        }
        if errors.is_empty() { Ok(Mission { triggers, objectives, every: self.ticks_per_second }) } else { Err(errors) }
    }

    fn cond(&self, v: &Value, path: &str, names: &Names, errors: &mut Vec<String>) -> Option<Cond<C>> {
        let (k, v) = single(v).map_err(|e| errors.push(format!("{path}: {e}"))).ok()?;
        let path = format!("{path}.{k}");
        // `Ok(None)`: a part had a fault, already reported.
        let read: Result<Option<Cond<C>>, String> = match k {
            "all" | "any" => match v.as_array() {
                Some(list) => {
                    let parts: Vec<_> = list
                        .iter()
                        .enumerate()
                        .map(|(i, c)| self.cond(c, &format!("{path}[{i}]"), names, errors))
                        .collect();
                    let parts: Option<Vec<_>> = parts.into_iter().collect();
                    Ok(parts.map(|p| if k == "all" { Cond::All(p) } else { Cond::Any(p) }))
                }
                None => Err(format!("must be a list, not {}", v.kind())),
            },
            "not" => Ok(self.cond(v, &path, names, errors).map(|c| Cond::Not(Box::new(c)))),
            "at_time" => seconds(v, self.ticks_per_second).map(|t| Some(Cond::AtTime(t))),
            "every" => self.every(v).map(Some),
            "fired" => names.trigger(v).map(|i| Some(Cond::Fired(i))),
            "objective" => {
                v.get("id").ok_or("needs an id".to_string()).and_then(|id| names.objective(id)).and_then(|i| {
                    let s = v.get("state").and_then(Value::as_str).and_then(ObjectiveState::from_id);
                    s.map(|s| Some(Cond::Objective(i, s))).ok_or("state must be hidden, open, done or failed".into())
                })
            }
            _ => match (self.cond)(k, v) {
                Some(r) => r.map(|c| Some(Cond::Engine(c))),
                None => Err("unknown condition".into()),
            },
        };
        read.map_err(|e| errors.push(format!("{path}: {e}"))).ok().flatten()
    }

    fn every(&self, v: &Value) -> Result<Cond<C>, String> {
        let (ticks, start) = match v {
            Value::Object(_) => (
                seconds(v.get("seconds").ok_or("needs seconds")?, self.ticks_per_second)?,
                v.get("start").map_or(Ok(0), |s| seconds(s, self.ticks_per_second))?,
            ),
            _ => (seconds(v, self.ticks_per_second)?, 0),
        };
        if ticks == 0 {
            return Err("seconds must be at least 1".into());
        }
        Ok(Cond::Every { ticks, start })
    }

    fn action(&self, v: &Value, path: &str, names: &Names, errors: &mut Vec<String>) -> Option<Action<A>> {
        let (k, v) = single(v).map_err(|e| errors.push(format!("{path}: {e}"))).ok()?;
        let read: Result<Action<A>, String> = match k {
            "set_objective" => (|| {
                let objective = names.objective(v.get("id").ok_or("needs an id")?)?;
                let text = v.get("text").and_then(Value::as_str).ok_or("needs a text id")?.to_string();
                let primary = match v.get("primary") {
                    None => true,
                    Some(p) => p.as_bool().ok_or("primary must be true or false")?,
                };
                Ok(Action::SetObjective { objective, text, primary })
            })(),
            "complete" => names.objective(v).map(Action::Complete),
            "fail" => names.objective(v).map(Action::Fail),
            "win" => self.player(v).map(Action::Win),
            "lose" => self.player(v).map(Action::Lose),
            "say" => {
                v.as_str().map(|l| Action::Say(l.to_string())).ok_or(format!("must be a line id, not {}", v.kind()))
            }
            _ => match (self.action)(k, v) {
                Some(r) => r.map(Action::Engine),
                None => Err("unknown action".into()),
            },
        };
        read.map_err(|e| errors.push(format!("{path}.{k}: {e}"))).ok()
    }
}

struct Names<'a> {
    triggers: &'a [String],
    objectives: &'a [String],
}

impl Names<'_> {
    fn find(list: &[String], what: &str, v: &Value) -> Result<usize, String> {
        let id = v.as_str().ok_or(format!("must be {what} id, not {}", v.kind()))?;
        list.iter().position(|x| x == id).ok_or(format!("no {what} {id}"))
    }

    fn trigger(&self, v: &Value) -> Result<usize, String> {
        Names::find(self.triggers, "trigger", v)
    }

    fn objective(&self, v: &Value) -> Result<usize, String> {
        Names::find(self.objectives, "objective", v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hash_of;
    use crate::json::parse;

    /// A stand-in engine: one condition (`flag`, true when the engine's flag of that number is up) and one action
    /// (`raise`, which raises a flag).
    fn read(text: &str) -> Result<Mission<u32, u32>, Vec<String>> {
        let cond = |k: &str, v: &Value| (k == "flag").then(|| v.as_int().map(|n| n as u32).ok_or("a number".into()));
        let action = |k: &str, v: &Value| (k == "raise").then(|| v.as_int().map(|n| n as u32).ok_or("a number".into()));
        let reader = Reader { ticks_per_second: 15, players: None, cond: &cond, action: &action };
        reader.triggers(&parse(text).expect("valid JSON"))
    }

    /// Run a mission for `ticks` ticks against a flag list the engine's actions raise; returns the state and every
    /// action fired, with its tick.
    fn run(m: &Mission<u32, u32>, ticks: u32, flags: &mut Vec<u32>) -> (MissionState, Vec<(u32, Action<u32>)>) {
        let mut state = MissionState::new(m);
        let mut log = Vec::new();
        for tick in 0..ticks {
            let fired = m.check(&mut state, tick, |f| flags.contains(f));
            for a in fired {
                if let Action::Engine(f) = a {
                    flags.push(*f);
                }
                log.push((tick, a.clone()));
            }
        }
        (state, log)
    }

    #[test]
    fn a_quota_mission_sets_its_objective_then_wins() {
        let m = read(
            r#"[
              { "id": "start", "when": { "at_time": 0 },
                "do": [{ "set_objective": { "id": "quota", "text": "obj.quota" } }, { "say": "adv.welcome" }] },
              { "id": "won", "when": { "all": [{ "flag": 7 }, { "objective": { "id": "quota", "state": "open" } }] },
                "do": [{ "complete": "quota" }, { "win": 1 }] }
            ]"#,
        )
        .expect("loads");
        assert_eq!(m.objectives, ["quota"]);
        let mut flags = Vec::new();
        let (state, log) = run(&m, 100, &mut flags);
        assert_eq!(state.objectives, [ObjectiveState::Open]);
        assert_eq!(state.shown[0], Some(("obj.quota".to_string(), true)));
        assert_eq!(log.len(), 2, "only the start trigger fired: {log:?}");
        flags.push(7);
        let mut state = state;
        let mut won_at = None;
        for tick in 100..200 {
            if !m.check(&mut state, tick, |f| flags.contains(f)).is_empty() {
                won_at.get_or_insert(tick);
            }
        }
        assert_eq!(won_at, Some(105), "the first check (once a second) after the flag went up");
        assert_eq!(state.objectives, [ObjectiveState::Done]);
        assert_eq!(state.outcome(0), Some(true));
        assert_eq!(state.outcome(1), None);
        assert_eq!(state.fired, [1, 1], "neither trigger repeats");
    }

    #[test]
    fn timed_repeating_and_chained_triggers_fire_on_their_ticks() {
        let m = read(
            r#"[
              { "id": "raid", "when": { "at_time": 4 }, "do": [{ "raise": 1 }] },
              { "id": "after", "when": { "fired": "raid" }, "do": [{ "raise": 2 }] },
              { "id": "pulse", "when": { "every": { "seconds": 3, "start": 2 } }, "do": [{ "raise": 3 }], "repeat": true },
              { "id": "never", "when": { "not": { "any": [{ "flag": 1 }, { "flag": 9 }] } }, "do": [{ "raise": 4 }] }
            ]"#,
        )
        .expect("loads");
        let mut flags = Vec::new();
        let (state, log) = run(&m, 15 * 10, &mut flags);
        let ticks = |n: u32| -> Vec<u32> {
            log.iter().filter(|(_, a)| *a == Action::Engine(n)).map(|(t, _)| *t).collect::<Vec<_>>()
        };
        assert_eq!(ticks(1), [60], "at 4 s");
        assert_eq!(ticks(2), [60], "a trigger sees one fired earlier in the same check");
        assert_eq!(ticks(3), [30, 75, 120], "every 3 s from 2 s");
        assert_eq!(ticks(4), [0], "flag 1 was down at the first check");
        assert_eq!(state.fired, [1, 1, 3, 1]);
    }

    #[test]
    fn the_first_end_for_a_player_stands() {
        let m = read(r#"[{ "id": "a", "when": { "at_time": 0 }, "do": [{ "lose": 2 }, { "win": 2 }, { "win": 1 }] }]"#)
            .expect("loads");
        let (state, _) = run(&m, 1, &mut Vec::new());
        assert_eq!(state.outcomes, [Outcome { player: 1, won: false }, Outcome { player: 0, won: true }]);
    }

    #[test]
    fn every_fault_is_reported_with_its_path() {
        let errors = read(
            r#"[
              { "id": "a", "when": { "at_time": -1 }, "do": [{ "win": 0 }] },
              { "id": "a", "when": { "all": [{ "flag": 1 }, { "wobble": 2 }] }, "do": [{ "complete": "nope" }] },
              { "id": "c", "when": { "fired": "zzz" }, "do": [{ "dance": 1 }], "extra": 1 }
            ]"#,
        )
        .expect_err("refused");
        assert_eq!(
            errors,
            [
                "triggers[1].id: a is used twice",
                "triggers[0].when.at_time: a time must be a whole number of seconds, not a number",
                "triggers[0].do[0].win: a player must be a number from 1, not a number",
                "triggers[1].when.all[1].wobble: unknown condition",
                "triggers[1].do[0].complete: no objective nope",
                "triggers[2].extra: unknown field",
                "triggers[2].when.fired: no trigger zzz",
                "triggers[2].do[0].dance: unknown action",
            ]
        );
    }

    #[test]
    fn a_trigger_id_used_twice_is_refused_not_a_panic() {
        let errors = read(
            r#"[{ "id": "a", "when": { "at_time": 0 }, "do": [{ "win": 1 }] },
                { "id": "a", "when": { "at_time": 1 }, "do": [{ "lose": 1 }] }]"#,
        )
        .expect_err("refused");
        assert_eq!(errors, ["triggers[1].id: a is used twice"]);
    }

    #[test]
    fn the_state_hashes_what_happened() {
        let m = read(r#"[{ "id": "a", "when": { "at_time": 1 }, "do": [{ "win": 1 }] }]"#).expect("loads");
        let before = MissionState::new(&m);
        let (after, _) = run(&m, 16, &mut Vec::new());
        assert_ne!(hash_of(&before).hex(), hash_of(&after).hex());
        assert_eq!(hash_of(&after).hex(), hash_of(&after.clone()).hex());
    }
}
