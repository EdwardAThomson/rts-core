//! Music that follows a game: a setting pack's tracks on the mixer's music bus, picked for the moment. Shared by both
//! engines' renderers, so one pack's music plays the same way in either; each engine only says when its local player
//! hits or loses something ([`Music::hit`], [`Music::loss`]) and how much time has passed ([`Music::step`]). Like the
//! rest of the platform layer it only reads the game; nothing here feeds back into the state.
//!
//! A pack's `audio/music.json` lists its tracks, each with a `file`, a `pool` and a `volume` from 0 to 100 (0 takes
//! it out of rotation). The pools are `menu` (the title screen), `calm` and `battle` (in a game), and `won` and
//! `lost` (played once on the end screen). The last pack with a `music.json` plays, the generic pack's only when the
//! pack has none, so no game mixes two packs' music.
//!
//! In a game the mood follows a tension value: each hit or loss adds to it, it drains as time passes, and the music
//! turns to `battle` once tension has stayed above `battle_above` for `battle_after_s`, and back to `calm` once it has
//! stayed below `calm_below` for `calm_after_s` (two thresholds and two waits, so a skirmish doesn't flip it back and
//! forth). Each engine keeps those numbers in its own `music.json` rules ([`MoodRules`]), since how often things are
//! hit differs between them. A change of track crossfades. A pool of one track loops it; a bigger pool plays its
//! tracks in turn, never the same twice running, fading into the next before one ends.
//!
//! Tracks are WAV, PCM or IMA ADPCM; the mixer keeps ADPCM compressed and decodes it as it plays, so a long track
//! costs no decoding when it loads.

use rts_core::json::{self, Value};

use crate::audio::{Mixer, TrackId, db};
use crate::files::Files;
use crate::wav;

/// A pack's music index, from the pack's folder.
pub const MUSIC_INDEX: &str = "audio/music.json";

/// What a track is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pool {
    Menu,
    Calm,
    Battle,
    Won,
    Lost,
}

impl Pool {
    pub const ALL: [Pool; 5] = [Pool::Menu, Pool::Calm, Pool::Battle, Pool::Won, Pool::Lost];

    pub fn id(self) -> &'static str {
        match self {
            Pool::Menu => "menu",
            Pool::Calm => "calm",
            Pool::Battle => "battle",
            Pool::Won => "won",
            Pool::Lost => "lost",
        }
    }
}

/// How the mood follows the fighting, from an engine's `music.json` rules.
#[derive(Clone, Debug, PartialEq)]
pub struct MoodRules {
    pub hit: f32,
    pub destroyed: f32,
    pub decay_per_second: f32,
    pub battle_above: f32,
    pub battle_after_s: f32,
    pub calm_below: f32,
    pub calm_after_s: f32,
    pub crossfade_s: f32,
    pub gain_db: f32,
}

impl MoodRules {
    pub fn parse(text: &str) -> Result<MoodRules, String> {
        let v = json::parse(text).map_err(|e| format!("music.json: {e:?}"))?;
        let n = |k: &str| -> Result<f32, String> {
            let x = v.get(k).ok_or_else(|| format!("music.json: no `{k}`"))?;
            x.as_int().map(|i| i as f32).ok_or_else(|| format!("music.json: `{k}` is not a whole number"))
        };
        Ok(MoodRules {
            hit: n("tension_per_hit")?,
            destroyed: n("tension_per_loss")?,
            decay_per_second: n("tension_drain_per_second")?,
            battle_above: n("battle_above")?,
            battle_after_s: n("battle_after_s")?,
            calm_below: n("calm_below")?,
            calm_after_s: n("calm_after_s")?,
            crossfade_s: n("crossfade_ms")? / 1000.0,
            gain_db: n("gain_db")?,
        })
    }
}

/// One of the pack's tracks.
#[derive(Clone, Debug)]
pub struct Piece {
    pub id: String,
    pub pool: Pool,
    pub track: TrackId,
    /// Linear, from the track's `volume`.
    pub gain: f32,
}

/// Every file the music index `index` names, for the browser build.
pub fn files_named(index: &str) -> Vec<String> {
    let Ok(v) = json::parse(index) else { return Vec::new() };
    v.get("tracks")
        .and_then(Value::as_object)
        .unwrap_or(&[])
        .iter()
        .filter_map(|(_, t)| t.get("file").and_then(Value::as_str).map(String::from))
        .collect()
}

/// Plays the music for one player's screen.
pub struct Music {
    pub rules: MoodRules,
    pub pieces: Vec<Piece>,
    pub tension: f32,
    /// `Calm` or `Battle` while a game is on.
    pub mood: Pool,
    /// How long tension has stayed across the threshold for the other mood, while it stays across.
    crossed_s: Option<f32>,
    /// The piece playing, and the one before, so a pool doesn't repeat itself.
    playing: Option<usize>,
    last: Option<usize>,
    /// For picking a track; the board's own, never the game's.
    rng: u64,
    pub warnings: Vec<String>,
}

impl Music {
    /// Load the music of the last of `packs` with a music index (later packs win, as for sounds).
    pub fn from_files(packs: &[Files], rules: MoodRules, seed: u64, mixer: &mut Mixer) -> Music {
        let mut warnings = Vec::new();
        let mut pieces = Vec::new();
        if let Some(files) = packs.iter().rev().find(|f| f.read_text(MUSIC_INDEX).is_ok()) {
            let text = files.read_text(MUSIC_INDEX).unwrap_or_default();
            match json::parse(&text).ok().as_ref().and_then(|v| v.get("tracks")?.as_object().map(|o| o.to_vec())) {
                Some(tracks) => {
                    for (id, t) in &tracks {
                        let at = format!("{}: {id}", files.name(MUSIC_INDEX));
                        let pool = t.get("pool").and_then(Value::as_str);
                        let Some(pool) = Pool::ALL.into_iter().find(|p| Some(p.id()) == pool) else {
                            warnings.push(format!("{at}: `pool` is one of menu, calm, battle, won or lost"));
                            continue;
                        };
                        let volume = t.get("volume").and_then(Value::as_int).unwrap_or(100);
                        if !(0..=100).contains(&volume) {
                            warnings.push(format!("{at}: `volume` runs from 0 to 100"));
                            continue;
                        }
                        let Some(file) = t.get("file").and_then(Value::as_str) else {
                            warnings.push(format!("{at}: no `file`"));
                            continue;
                        };
                        match files
                            .read(file)
                            .and_then(|b| wav::decode_track(&b).map_err(|e| format!("{}: {e}", files.name(file))))
                        {
                            Ok(track) if volume > 0 => pieces.push(Piece {
                                id: id.clone(),
                                pool,
                                track: mixer.add_track(track),
                                gain: volume as f32 / 100.0,
                            }),
                            Ok(_) => {}
                            Err(e) => warnings.push(e),
                        }
                    }
                }
                None => warnings.push(format!("{}: needs a `tracks` object", files.name(MUSIC_INDEX))),
            }
        }
        Music {
            rules,
            pieces,
            tension: 0.0,
            mood: Pool::Calm,
            crossed_s: None,
            playing: None,
            last: None,
            rng: seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1,
            warnings,
        }
    }

    fn roll(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    /// The piece playing, by id.
    pub fn playing(&self) -> Option<&str> {
        self.playing.map(|i| self.pieces[i].id.as_str())
    }

    /// Keep a track from `pool` playing: start one if none is, the music is from another pool, or the one playing
    /// is about to end (a pool of several plays them in turn). `again` false plays a pool once, for the stingers.
    fn keep(&mut self, pool: Pool, again: bool, mixer: &mut Mixer) {
        let fade = self.rules.crossfade_s;
        let same_pool = self.playing.is_some_and(|i| self.pieces[i].pool == pool);
        let current = self.playing.filter(|_| same_pool).map(|i| self.pieces[i].track);
        let live = current.is_some() && mixer.music() == current;
        let several = self.pieces.iter().filter(|p| p.pool == pool).count() > 1;
        // Fade into the next track before this one ends, when there is a next.
        let ending = live
            && again
            && several
            && current.is_some_and(|t| {
                let len = mixer.track(t).seconds();
                mixer.music_at().is_some_and(|at| at >= len - fade) && len > 2.0 * fade
            });
        if live && !ending {
            return;
        }
        if same_pool && !again {
            // A stinger plays once.
            return;
        }
        let choices: Vec<usize> = (0..self.pieces.len()).filter(|&i| self.pieces[i].pool == pool).collect();
        if choices.is_empty() {
            if self.playing.is_some() {
                mixer.stop_music(fade);
                self.playing = None;
            }
            return;
        }
        // Never the same track twice running when there is another.
        let fresh: Vec<usize> = choices.iter().copied().filter(|&i| Some(i) != self.playing).collect();
        let from = if fresh.is_empty() { &choices } else { &fresh };
        let pick = from[(self.roll() % from.len() as u64) as usize];
        let p = &self.pieces[pick];
        // A pool of one loops its track; a bigger pool moves on to another at the end.
        let looping = again && choices.len() == 1;
        mixer.play_music(p.track, p.gain * db(self.rules.gain_db), fade, looping);
        self.last = self.playing;
        self.playing = Some(pick);
    }

    /// The title screen: the `menu` pool. Call once a frame while it shows.
    pub fn title(&mut self, mixer: &mut Mixer) {
        self.keep(Pool::Menu, true, mixer);
    }

    /// The end screen: the `won` or `lost` stinger, once, then quiet. Call once a frame while it shows.
    pub fn over(&mut self, won: bool, mixer: &mut Mixer) {
        self.keep(if won { Pool::Won } else { Pool::Lost }, false, mixer);
    }

    /// Fade the music out (the player muted it, or a game is loading).
    pub fn stop(&mut self, mixer: &mut Mixer) {
        mixer.stop_music(self.rules.crossfade_s);
        self.playing = None;
    }

    /// The local player hit something, or something hit them.
    pub fn hit(&mut self) {
        self.tension += self.rules.hit;
    }

    /// The local player lost something, or destroyed something of another player's.
    pub fn loss(&mut self) {
        self.tension += self.rules.destroyed;
    }

    /// `seconds` of game time have passed since the last step: drain the tension, move the mood, and keep the mood's
    /// music playing. Call once a game tick, after telling it that tick's hits and losses.
    pub fn step(&mut self, seconds: f32, mixer: &mut Mixer) {
        self.tension = (self.tension - self.rules.decay_per_second * seconds).max(0.0);
        let (across, wait) = match self.mood {
            Pool::Battle => (self.tension < self.rules.calm_below, self.rules.calm_after_s),
            _ => (self.tension > self.rules.battle_above, self.rules.battle_after_s),
        };
        // The first step across starts the wait; the mood turns once the wait has passed.
        self.crossed_s = match (across, self.crossed_s) {
            (false, _) => None,
            (true, None) => Some(0.0),
            (true, Some(s)) if s + seconds >= wait => {
                self.mood = if self.mood == Pool::Battle { Pool::Calm } else { Pool::Battle };
                None
            }
            (true, Some(s)) => Some(s + seconds),
        };
        self.keep(self.mood, true, mixer);
    }

    /// A new game, or a loaded one played forward to where it was saved: calm, with no tension.
    pub fn new_game(&mut self) {
        self.tension = 0.0;
        self.mood = Pool::Calm;
        self.crossed_s = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &str = r#"{ "tension_per_hit": 2, "tension_per_loss": 6, "tension_drain_per_second": 3,
        "battle_above": 20, "battle_after_s": 3, "calm_below": 6, "calm_after_s": 20, "crossfade_ms": 2000,
        "gain_db": 0 }"#;

    #[test]
    fn the_mood_turns_to_battle_after_a_wait_and_back_after_a_longer_one() {
        let mut mixer = Mixer::new(48_000);
        let mut m = Music::from_files(&[], MoodRules::parse(RULES).unwrap(), 1, &mut mixer);
        let tick = 1.0 / 15.0;
        // A fight: a loss every tick for four seconds.
        let mut turned = None;
        for i in 0..60 {
            m.loss();
            m.step(tick, &mut mixer);
            if m.mood == Pool::Battle && turned.is_none() {
                turned = Some(i);
            }
        }
        // Over the threshold within a few ticks, then three seconds' wait.
        let turned = turned.expect("the fight turns the music to battle");
        assert!((45..55).contains(&turned), "turned at tick {turned}");
        // Quiet: the tension drains below calm_below, then twenty seconds pass before the music calms.
        let below = ((m.tension - m.rules.calm_below) / (m.rules.decay_per_second * tick)).ceil() as i32;
        let mut calm = None;
        for i in 0..below + 600 {
            m.step(tick, &mut mixer);
            if m.mood == Pool::Calm && calm.is_none() {
                calm = Some(i);
            }
        }
        let calm = calm.expect("the music calms down");
        assert!((below + 295..below + 305).contains(&calm), "calmed at tick {calm}, tension below 6 from {below}");
    }

    #[test]
    fn a_bad_track_is_warned_about_and_left_out() {
        let index = r#"{ "tracks": { "a": { "file": "x.wav", "pool": "dance" }, "b": { "pool": "calm" },
            "c": { "file": "missing.wav", "pool": "calm", "volume": 50 }, "d": { "file": "y.wav", "volume": 101,
            "pool": "menu" } } }"#;
        let mut files = std::collections::BTreeMap::new();
        files.insert(MUSIC_INDEX.to_string(), index.as_bytes().to_vec());
        let pack = Files::Memory { label: "pack".into(), files };
        let mut mixer = Mixer::new(48_000);
        let m = Music::from_files(&[pack], MoodRules::parse(RULES).unwrap(), 1, &mut mixer);
        assert!(m.pieces.is_empty());
        assert_eq!(m.warnings.len(), 4, "{:?}", m.warnings);
    }
}
