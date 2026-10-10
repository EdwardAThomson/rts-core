//! A small software mixer and the sound device it feeds. Genre-neutral: it plays clips on buses and knows nothing
//! about games, so both engines share it.
//!
//! The mixer is pure: `render` fills a buffer, so tests run it without a device, and the same code can later sit
//! behind a browser's audio callback. `Speaker` (the `device` feature) pulls from it on the sound card's thread.
//!
//! Limits follow the playbooks' audio design (`plans/rts/audio.md`): a cap on voices in all, a cap per sound, the
//! same sound started again within 40 ms merged into the one already playing (a little louder), and when a cap is
//! hit the quietest, least important voice gives way, or the new sound is dropped if it would be that voice.
//!
//! Besides one-shot sounds it plays **loops** (an engine, a drill: `start_loop`, then `set_loop` to follow the
//! source and `stop_loop`), which fade in and out, glide to each new level and place, sit outside the one-shot caps
//! and have a cap of their own (`max_loops`); and **music** on the music bus (`play_music`), long tracks read a
//! block at a time as they play, with a crossfade from one track to the next.

use super::wav::{Clip, Track};

/// Mix groups, each with its own volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bus {
    /// Weapons, impacts, explosions, building sounds.
    Sfx = 0,
    /// Interface sounds: clicks, blips, errors. Never stolen by sfx.
    Ui = 1,
    /// Unit replies and announcements.
    Voice = 2,
    Music = 3,
}

impl Bus {
    pub const ALL: [Bus; 4] = [Bus::Sfx, Bus::Ui, Bus::Voice, Bus::Music];

    pub fn from_id(id: &str) -> Option<Bus> {
        Bus::ALL.into_iter().find(|b| b.id() == id)
    }

    pub fn id(self) -> &'static str {
        match self {
            Bus::Sfx => "sfx",
            Bus::Ui => "ui",
            Bus::Voice => "voice",
            Bus::Music => "music",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClipId(pub u32);

/// A loop the mixer is playing, from [`Mixer::start_loop`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LoopId(pub u32);

/// A piece of music added with [`Mixer::add_track`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TrackId(pub u32);

/// One request to play a clip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sound {
    pub clip: ClipId,
    /// Which sound this is, for the per-sound cap and merging: several clips (takes) can share one key.
    pub key: u32,
    pub bus: Bus,
    /// Linear gain, 1 for as recorded.
    pub gain: f32,
    /// -1 hard left to 1 hard right.
    pub pan: f32,
    /// Playback speed, 1 for as recorded; it shifts the pitch too.
    pub speed: f32,
    /// Higher wins when a cap forces a choice.
    pub priority: i32,
    /// How many of this key may play at once.
    pub max_instances: u32,
}

/// What `Mixer::play` did with a sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Played {
    Started,
    /// Folded into the same sound started a moment ago.
    Merged,
    /// Took the place of a quieter or less important voice.
    Stole,
    /// Lost to every voice already playing, or muted.
    Dropped,
}

struct Voice {
    sound: Sound,
    /// Position in the clip, in clip samples.
    at: f64,
    started: u64,
    /// The gain it started with, before any merging.
    base_gain: f32,
    /// Output samples left in a fade-out, for a stolen voice.
    fading: Option<u32>,
    /// A loop's id: it plays round and round until stopped, outside the one-shot caps.
    looping: Option<u32>,
    /// A loop's fade, from 0 to 1, where it is heading and how far it moves each output sample.
    env: f32,
    env_to: f32,
    env_step: f32,
    /// The gain and pan a loop glides to over the next rendered buffer.
    gain_to: f32,
    pan_to: f32,
}

impl Voice {
    fn new(sound: Sound, started: u64) -> Voice {
        Voice {
            sound,
            at: 0.0,
            started,
            base_gain: sound.gain,
            fading: None,
            looping: None,
            env: 1.0,
            env_to: 1.0,
            env_step: 0.0,
            gain_to: sound.gain,
            pan_to: sound.pan,
        }
    }

    /// Counts against the one-shot caps: playing, not fading out, not a loop.
    fn one_shot(&self) -> bool {
        self.fading.is_none() && self.looping.is_none()
    }
}

/// A track playing on the music bus. Two can play at once, while one fades into the next.
struct Deck {
    track: TrackId,
    /// Position in the track, in its frames.
    at: f64,
    gain: f32,
    looping: bool,
    /// The fade, as for a loop: 0 to 1, its target and its step per output sample.
    env: f32,
    env_to: f32,
    env_step: f32,
    /// The two blocks decoded last, by block number: a block and the next, which interpolation reaches into.
    cache: [(usize, Vec<f32>); 2],
}

impl Deck {
    /// Frame `i`'s left and right samples, decoding its block if needed.
    fn frame(&mut self, track: &Track, i: usize) -> (f32, f32) {
        let per = track.per_block();
        let k = i / per;
        let slot = &mut self.cache[k % 2];
        if slot.0 != k || slot.1.is_empty() {
            slot.0 = k;
            track.block(k, &mut slot.1);
        }
        let ch = track.channels as usize;
        let j = (i - k * per) * ch;
        match (slot.1.get(j), slot.1.get(j + ch - 1)) {
            (Some(&l), Some(&r)) => (l, r),
            _ => (0.0, 0.0),
        }
    }
}

/// Moves `env` one output sample towards `to` by `step`.
fn approach(env: &mut f32, to: f32, step: f32) {
    if *env < to {
        *env = (*env + step).min(to);
    } else if *env > to {
        *env = (*env - step).max(to);
    }
}

/// The per-sample step that covers 0 to 1 in `secs` at `rate`; instant for no time.
fn ramp(secs: f32, rate: u32) -> f32 {
    if secs <= 0.0 { 1.0 } else { 1.0 / (secs * rate as f32).max(1.0) }
}

/// Merge window and steal fade, in seconds.
const MERGE: f32 = 0.040;
const FADE: f32 = 0.020;

pub struct Mixer {
    /// Output samples per second.
    pub rate: u32,
    clips: Vec<Clip>,
    voices: Vec<Voice>,
    tracks: Vec<Track>,
    decks: Vec<Deck>,
    next_loop: u32,
    /// Output frames rendered so far: the mixer's clock.
    now: u64,
    pub bus_gain: [f32; 4],
    pub master: f32,
    pub muted: bool,
    /// Voices playing at once, all buses together (fading ones and loops aside).
    pub max_voices: usize,
    /// Loops playing at once (fading-out ones aside).
    pub max_loops: usize,
}

impl Mixer {
    pub fn new(rate: u32) -> Mixer {
        Mixer {
            rate,
            clips: Vec::new(),
            voices: Vec::new(),
            tracks: Vec::new(),
            decks: Vec::new(),
            next_loop: 0,
            now: 0,
            // Starting levels from the design: music -6 dB, ui -3 dB.
            bus_gain: [1.0, db(-3.0), 1.0, db(-6.0)],
            master: 1.0,
            muted: false,
            max_voices: 24,
            max_loops: 6,
        }
    }

    pub fn add_clip(&mut self, clip: Clip) -> ClipId {
        self.clips.push(clip);
        ClipId(self.clips.len() as u32 - 1)
    }

    pub fn clip(&self, id: ClipId) -> &Clip {
        &self.clips[id.0 as usize]
    }

    /// One-shot voices playing, not counting ones fading out.
    pub fn playing(&self) -> usize {
        self.voices.iter().filter(|v| v.one_shot()).count()
    }

    pub fn playing_key(&self, key: u32) -> usize {
        self.voices.iter().filter(|v| v.one_shot() && v.sound.key == key).count()
    }

    fn score(s: &Sound) -> f32 {
        s.priority as f32 + 20.0 * s.gain
    }

    pub fn play(&mut self, sound: Sound) -> Played {
        if self.muted || sound.gain <= 0.0 || self.clips.get(sound.clip.0 as usize).is_none_or(|c| c.samples.is_empty())
        {
            return Played::Dropped;
        }
        let merge = (MERGE * self.rate as f32) as u64;
        let now = self.now;
        if let Some(v) =
            self.voices.iter_mut().find(|v| v.one_shot() && v.sound.key == sound.key && now - v.started <= merge)
        {
            // A little louder each time, up to 3 dB over the louder of the first two.
            let base = v.base_gain.max(sound.gain);
            v.base_gain = base;
            v.sound.gain = (v.sound.gain.max(sound.gain) * db(1.0)).min(base * db(3.0));
            return Played::Merged;
        }
        // The voice that would give way: first among this sound's own, then among all on buses that can be stolen.
        let full_key = self.playing_key(sound.key) >= sound.max_instances.max(1) as usize;
        let full_all = self.playing() >= self.max_voices;
        let mut stolen = false;
        if full_key || full_all {
            let victim = self
                .voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.one_shot())
                .filter(|(_, v)| if full_key { v.sound.key == sound.key } else { v.sound.bus != Bus::Ui })
                .min_by(|a, b| Self::score(&a.1.sound).total_cmp(&Self::score(&b.1.sound)))
                .map(|(i, v)| (i, Self::score(&v.sound)));
            // Interface sounds always play: over the total cap they take an sfx voice's place whatever its score,
            // or go over the cap when there is none.
            let ui = sound.bus == Bus::Ui && !full_key;
            match victim {
                Some((i, s)) if ui || s < Self::score(&sound) => {
                    self.voices[i].fading = Some((FADE * self.rate as f32) as u32);
                    stolen = true;
                }
                None if ui => {}
                _ => return Played::Dropped,
            }
        }
        self.voices.push(Voice::new(sound, now));
        if stolen { Played::Stole } else { Played::Started }
    }

    /// Loops playing, not counting ones fading out.
    pub fn loops(&self) -> usize {
        self.voices.iter().filter(|v| v.looping.is_some() && v.env_to > 0.0).count()
    }

    /// Whether loop `id` still plays (and isn't fading out): a stolen or stopped loop doesn't.
    pub fn looping(&self, id: LoopId) -> bool {
        self.voices.iter().any(|v| v.looping == Some(id.0) && v.env_to > 0.0)
    }

    /// Start `sound` playing round and round, fading in over `fade` seconds. Over `max_loops`, the loop with the
    /// lowest score gives way if the new one beats it; otherwise, or when muted or with no clip, it doesn't start.
    pub fn start_loop(&mut self, sound: Sound, fade: f32) -> Option<LoopId> {
        if self.muted || self.clips.get(sound.clip.0 as usize).is_none_or(|c| c.samples.len() < 2) {
            return None;
        }
        if self.loops() >= self.max_loops.max(1) {
            let (i, worst) = self
                .voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.looping.is_some() && v.env_to > 0.0)
                .map(|(i, v)| (i, Self::score(&v.sound)))
                .min_by(|a, b| a.1.total_cmp(&b.1))?;
            if worst >= Self::score(&sound) {
                return None;
            }
            let v = &mut self.voices[i];
            v.env_to = 0.0;
            v.env_step = ramp(FADE, self.rate);
        }
        let id = self.next_loop;
        self.next_loop = self.next_loop.wrapping_add(1);
        let mut v = Voice::new(sound, self.now);
        v.looping = Some(id);
        v.env = 0.0;
        v.env_step = ramp(fade, self.rate);
        self.voices.push(v);
        Some(LoopId(id))
    }

    /// Move loop `id` to a new level and place, gliding there over the next buffer.
    pub fn set_loop(&mut self, id: LoopId, gain: f32, pan: f32) {
        if let Some(v) = self.voices.iter_mut().find(|v| v.looping == Some(id.0)) {
            v.gain_to = gain.max(0.0);
            v.pan_to = pan.clamp(-1.0, 1.0);
        }
    }

    /// Fade loop `id` out over `fade` seconds, then let it go.
    pub fn stop_loop(&mut self, id: LoopId, fade: f32) {
        if let Some(v) = self.voices.iter_mut().find(|v| v.looping == Some(id.0)) {
            v.env_to = 0.0;
            v.env_step = ramp(fade, self.rate);
        }
    }

    pub fn add_track(&mut self, track: Track) -> TrackId {
        self.tracks.push(track);
        TrackId(self.tracks.len() as u32 - 1)
    }

    pub fn track(&self, id: TrackId) -> &Track {
        &self.tracks[id.0 as usize]
    }

    /// Play `track` on the music bus at linear `gain`, crossfading from whatever plays over `fade` seconds (equal
    /// power, so the sum doesn't dip). A `looping` track starts over at its end; any other stops there.
    pub fn play_music(&mut self, track: TrackId, gain: f32, fade: f32, looping: bool) {
        if self.tracks.get(track.0 as usize).is_none() {
            return;
        }
        self.stop_music(fade);
        let step = ramp(fade, self.rate);
        let cache = [(usize::MAX, Vec::new()), (usize::MAX, Vec::new())];
        self.decks.push(Deck { track, at: 0.0, gain, looping, env: 0.0, env_to: 1.0, env_step: step, cache });
    }

    /// Fade the music out over `fade` seconds.
    pub fn stop_music(&mut self, fade: f32) {
        let step = ramp(fade, self.rate);
        for d in &mut self.decks {
            d.env_to = 0.0;
            d.env_step = step;
        }
    }

    /// The track playing, unless it is fading out or has come to its end.
    pub fn music(&self) -> Option<TrackId> {
        self.decks.iter().rev().find(|d| d.env_to > 0.0).map(|d| d.track)
    }

    /// How far into the current track the music is, in seconds.
    pub fn music_at(&self) -> Option<f32> {
        let d = self.decks.iter().rev().find(|d| d.env_to > 0.0)?;
        Some(d.at as f32 / self.tracks[d.track.0 as usize].rate as f32)
    }

    /// Stop everything at once: sounds, loops and music.
    pub fn stop_all(&mut self) {
        self.voices.clear();
        self.decks.clear();
    }

    /// Fill `out`, interleaved with `channels` channels, and advance the clock. Stereo goes to the first two
    /// channels; a mono device gets the two averaged; any further channels stay silent.
    pub fn render(&mut self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        let channels = channels.max(1);
        let frames = out.len() / channels;
        let master = if self.muted { 0.0 } else { self.master };
        // Constant-power pan.
        let sides = |gain: f32, pan: f32| {
            let pan = pan.clamp(-1.0, 1.0);
            (((1.0 - pan) / 2.0).sqrt() * gain, ((1.0 + pan) / 2.0).sqrt() * gain)
        };
        for v in &mut self.voices {
            let clip = &self.clips[v.sound.clip.0 as usize];
            let len = clip.samples.len();
            let step = clip.rate as f64 / self.rate as f64 * v.sound.speed.max(0.01) as f64;
            let bus = self.bus_gain[v.sound.bus as usize] * master;
            // A loop glides from where it was to where it was last set, over this buffer.
            let (l0, r0) = sides(v.sound.gain * bus, v.sound.pan);
            let (l1, r1) = if v.looping.is_some() { sides(v.gain_to * bus, v.pan_to) } else { (l0, r0) };
            if v.looping.is_some() {
                v.sound.gain = v.gain_to;
                v.sound.pan = v.pan_to;
            }
            let fade_len = (FADE * self.rate as f32).max(1.0);
            for f in 0..frames {
                let i = v.at as usize;
                let t = (v.at - i as f64) as f32;
                let mut s = if v.looping.is_some() {
                    approach(&mut v.env, v.env_to, v.env_step);
                    if v.env <= 0.0 && v.env_to <= 0.0 {
                        break;
                    }
                    (clip.samples[i % len] * (1.0 - t) + clip.samples[(i + 1) % len] * t) * v.env
                } else {
                    if i + 1 >= len {
                        v.at = len as f64;
                        break;
                    }
                    clip.samples[i] * (1.0 - t) + clip.samples[i + 1] * t
                };
                if let Some(left) = &mut v.fading {
                    if *left == 0 {
                        break;
                    }
                    s *= *left as f32 / fade_len;
                    *left -= 1;
                }
                let k = f as f32 / frames.max(1) as f32;
                let (gl, gr) = (l0 + (l1 - l0) * k, r0 + (r1 - r0) * k);
                let o = &mut out[f * channels..(f + 1) * channels];
                if channels == 1 {
                    o[0] += s * (gl + gr) / 2.0;
                } else {
                    o[0] += s * gl;
                    o[1] += s * gr;
                }
                v.at += step;
                if v.looping.is_some() && v.at >= len as f64 {
                    v.at -= len as f64;
                }
            }
        }
        let clips = &self.clips;
        self.voices.retain(|v| match v.looping {
            Some(_) => v.env > 0.0 || v.env_to > 0.0,
            None => v.fading != Some(0) && (v.at as usize) + 1 < clips[v.sound.clip.0 as usize].samples.len(),
        });
        let bus = self.bus_gain[Bus::Music as usize] * master;
        for d in &mut self.decks {
            let track = &self.tracks[d.track.0 as usize];
            let step = track.rate as f64 / self.rate as f64;
            let end = track.frames;
            if end < 2 {
                d.env = 0.0;
                d.env_to = 0.0;
                continue;
            }
            for f in 0..frames {
                approach(&mut d.env, d.env_to, d.env_step);
                if d.env <= 0.0 && d.env_to <= 0.0 {
                    break;
                }
                let i = d.at as usize;
                if i + 1 >= end && !d.looping {
                    d.env = 0.0;
                    d.env_to = 0.0;
                    break;
                }
                let t = (d.at - i as f64) as f32;
                let (a, b) = (d.frame(track, i), d.frame(track, (i + 1) % end));
                // Equal power: the fade's square root, so two uncorrelated tracks crossing keep their loudness.
                let g = d.env.sqrt() * d.gain * bus;
                let (l, r) = ((a.0 * (1.0 - t) + b.0 * t) * g, (a.1 * (1.0 - t) + b.1 * t) * g);
                let o = &mut out[f * channels..(f + 1) * channels];
                if channels == 1 {
                    o[0] += (l + r) / 2.0;
                } else {
                    o[0] += l;
                    o[1] += r;
                }
                d.at += step;
                if d.at >= end as f64 {
                    d.at -= end as f64;
                }
            }
        }
        self.decks.retain(|d| d.env > 0.0 || d.env_to > 0.0);
        for s in out.iter_mut() {
            *s = limit(*s);
        }
        self.now += frames as u64;
    }
}

/// Decibels to linear gain.
pub fn db(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// A soft limiter: untouched below 0.8, then bending smoothly towards 1 so a big battle never clips.
fn limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.8 { x } else { x.signum() * (0.8 + 0.2 * ((a - 0.8) / 0.2).tanh()) }
}

/// The sound card: a stream that pulls from a shared mixer on the device's own thread.
#[cfg(feature = "device")]
pub struct Speaker {
    _stream: cpal::Stream,
    pub describe: String,
}

#[cfg(feature = "device")]
impl Speaker {
    /// Open the default output device and start pulling from `mixer`, whose rate is set to the device's.
    pub fn open(mixer: std::sync::Arc<std::sync::Mutex<Mixer>>) -> Result<Speaker, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no sound output device")?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let format = config.sample_format();
        let config: cpal::StreamConfig = config.into();
        mixer.lock().map_err(|e| e.to_string())?.rate = config.sample_rate;
        let describe = format!("{} Hz, {} channels, {format:?}", config.sample_rate, config.channels);
        let stream = match format {
            cpal::SampleFormat::F32 => stream::<f32>(&device, &config, mixer),
            cpal::SampleFormat::I16 => stream::<i16>(&device, &config, mixer),
            cpal::SampleFormat::U16 => stream::<u16>(&device, &config, mixer),
            cpal::SampleFormat::I32 => stream::<i32>(&device, &config, mixer),
            other => return Err(format!("unsupported sample format {other:?}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Speaker { _stream: stream, describe })
    }
}

#[cfg(feature = "device")]
fn stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mixer: std::sync::Arc<std::sync::Mutex<Mixer>>,
) -> Result<cpal::Stream, String> {
    use cpal::traits::DeviceTrait;
    let channels = config.channels as usize;
    let mut buf = Vec::new();
    device
        .build_output_stream(
            *config,
            move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
                buf.resize(out.len(), 0.0);
                match mixer.lock() {
                    Ok(mut m) => m.render(&mut buf, channels),
                    Err(_) => buf.fill(0.0),
                }
                for (o, s) in out.iter_mut().zip(&buf) {
                    *o = T::from_sample(*s);
                }
            },
            |e| eprintln!("sound output: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(mixer: &mut Mixer, len: usize) -> ClipId {
        mixer.add_clip(Clip { rate: 1000, samples: (0..len).map(|i| if i % 2 == 0 { 0.5 } else { -0.5 }).collect() })
    }

    fn sound(clip: ClipId, key: u32) -> Sound {
        Sound { clip, key, bus: Bus::Sfx, gain: 1.0, pan: 0.0, speed: 1.0, priority: 50, max_instances: 3 }
    }

    #[test]
    fn plays_to_the_end_then_frees_the_voice() {
        let mut m = Mixer::new(1000);
        let c = tone(&mut m, 100);
        assert_eq!(m.play(sound(c, 0)), Played::Started);
        let mut out = vec![0.0; 2 * 50];
        m.render(&mut out, 2);
        assert!(out.iter().any(|&s| s != 0.0));
        assert_eq!(m.playing(), 1);
        m.render(&mut out, 2);
        assert_eq!(m.playing(), 0);
    }

    #[test]
    fn pan_moves_the_sound_between_channels() {
        let mut m = Mixer::new(1000);
        let c = tone(&mut m, 100);
        m.play(Sound { pan: -1.0, ..sound(c, 0) });
        let mut out = vec![0.0; 2 * 10];
        m.render(&mut out, 2);
        assert!(out.chunks(2).all(|f| f[1].abs() < 1e-6) && out.chunks(2).any(|f| f[0] != 0.0));
    }

    #[test]
    fn same_sound_at_once_merges_and_caps_hold() {
        let mut m = Mixer::new(1000);
        let c = tone(&mut m, 1000);
        assert_eq!(m.play(sound(c, 7)), Played::Started);
        assert_eq!(m.play(sound(c, 7)), Played::Merged);
        assert_eq!(m.playing(), 1);
        // Spread out past the merge window: the per-sound cap of 3 holds, equal scores never steal.
        let mut out = vec![0.0; 2 * 50];
        for _ in 0..5 {
            m.render(&mut out, 2);
            m.play(sound(c, 7));
        }
        assert_eq!(m.playing_key(7), 3);
        // A louder one takes the place of a quieter one.
        m.render(&mut out, 2);
        assert_eq!(m.play(Sound { priority: 90, ..sound(c, 7) }), Played::Stole);
        assert_eq!(m.playing_key(7), 3);
    }

    #[test]
    fn the_voice_cap_spares_the_interface() {
        let mut m = Mixer::new(1000);
        m.max_voices = 4;
        let c = tone(&mut m, 1000);
        for key in 0..10 {
            m.play(sound(c, key));
        }
        assert_eq!(m.playing(), 4);
        assert_eq!(m.play(Sound { bus: Bus::Ui, ..sound(c, 99) }), Played::Stole);
        assert_eq!(m.playing(), 4);
        // With only ui sounds left, another ui sound still plays.
        m.stop_all();
        for key in 0..4 {
            m.play(Sound { bus: Bus::Ui, ..sound(c, 100 + key) });
        }
        assert_eq!(m.play(Sound { bus: Bus::Ui, ..sound(c, 200) }), Played::Started);
    }

    #[test]
    fn a_crowd_never_clips() {
        let mut m = Mixer::new(1000);
        let c = m.add_clip(Clip { rate: 1000, samples: vec![1.0; 200] });
        for key in 0..24 {
            m.play(Sound { gain: 2.0, ..sound(c, key) });
        }
        let mut out = vec![0.0; 2 * 100];
        m.render(&mut out, 2);
        assert!(out.iter().all(|s| s.abs() <= 1.0));
    }

    #[test]
    fn a_loop_goes_round_until_stopped_then_fades_out() {
        let mut m = Mixer::new(1000);
        let c = tone(&mut m, 100);
        let id = m.start_loop(sound(c, 5), 0.01).unwrap();
        let mut out = vec![0.0; 2 * 50];
        for _ in 0..10 {
            m.render(&mut out, 2);
        }
        // Five times through the clip, and still going; it isn't a one-shot voice.
        assert!(m.looping(id) && m.loops() == 1 && m.playing() == 0);
        assert!(out.iter().any(|&s| s != 0.0));
        m.set_loop(id, 0.5, -1.0);
        m.render(&mut out, 2);
        m.render(&mut out, 2);
        assert!(out.chunks(2).all(|f| f[1].abs() < 1e-6), "panned hard left");
        m.stop_loop(id, 0.02);
        assert!(!m.looping(id));
        m.render(&mut out, 2);
        assert_eq!(m.loops(), 0);
        m.render(&mut out, 2);
        assert!(out.iter().all(|&s| s == 0.0), "faded out and gone");
    }

    #[test]
    fn the_loop_cap_keeps_the_loudest() {
        let mut m = Mixer::new(1000);
        m.max_loops = 2;
        let c = tone(&mut m, 100);
        let quiet = m.start_loop(Sound { gain: 0.2, ..sound(c, 1) }, 0.0).unwrap();
        let loud = m.start_loop(sound(c, 2), 0.0).unwrap();
        // A quieter third doesn't start; a louder one takes the quiet one's place.
        assert_eq!(m.start_loop(Sound { gain: 0.1, ..sound(c, 3) }, 0.0), None);
        let louder = m.start_loop(Sound { priority: 90, ..sound(c, 4) }, 0.0).unwrap();
        assert!(!m.looping(quiet) && m.looping(loud) && m.looping(louder));
        assert_eq!(m.loops(), 2);
    }

    fn music_track(m: &mut Mixer, value: i16, frames: usize) -> TrackId {
        let bytes = super::super::wav::encode_adpcm(1000, 1, &vec![value; frames], 36);
        m.add_track(super::super::wav::decode_track(&bytes).unwrap())
    }

    #[test]
    fn music_crossfades_from_one_track_to_the_next() {
        let mut m = Mixer::new(1000);
        m.bus_gain = [1.0; 4];
        let a = music_track(&mut m, 8000, 3000);
        let b = music_track(&mut m, -8000, 3000);
        m.play_music(a, 1.0, 0.0, true);
        let mut out = vec![0.0; 2 * 100];
        m.render(&mut out, 2);
        assert!(out.iter().all(|&s| s > 0.0), "the first track, at once");
        m.play_music(b, 1.0, 0.2, true);
        assert_eq!(m.music(), Some(b));
        m.render(&mut out, 2);
        // Half way through the fade both are heard; afterwards only the second.
        let (first, last) = (out[0], out[out.len() - 1]);
        assert!(first > 0.0 && last < first);
        m.render(&mut out, 2);
        m.render(&mut out, 2);
        assert!(out.iter().all(|&s| s < 0.0), "the second track only");
        assert_eq!(m.decks.len(), 1);
    }

    #[test]
    fn music_that_doesnt_loop_stops_at_its_end() {
        let mut m = Mixer::new(1000);
        let a = music_track(&mut m, 8000, 150);
        m.play_music(a, 1.0, 0.0, false);
        let mut out = vec![0.0; 2 * 100];
        m.render(&mut out, 2);
        assert_eq!(m.music(), Some(a));
        assert!(m.music_at().unwrap() > 0.09);
        m.render(&mut out, 2);
        assert_eq!(m.music(), None);
        let looping = music_track(&mut m, 8000, 150);
        m.play_music(looping, 1.0, 0.0, true);
        for _ in 0..5 {
            m.render(&mut out, 2);
        }
        assert_eq!(m.music(), Some(looping));
    }

    #[test]
    fn muted_drops_everything() {
        let mut m = Mixer::new(1000);
        let c = tone(&mut m, 100);
        m.muted = true;
        assert_eq!(m.play(sound(c, 0)), Played::Dropped);
    }
}
