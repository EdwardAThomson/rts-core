//! A small software mixer and the sound device it feeds. Genre-neutral: it plays clips on buses and knows nothing
//! about games, so both engines share it.
//!
//! The mixer is pure: `render` fills a buffer, so tests run it without a device, and the same code can later sit
//! behind a browser's audio callback. `Speaker` (the `device` feature) pulls from it on the sound card's thread.
//!
//! Limits follow the playbooks' audio design (`plans/rts/audio.md`): a cap on voices in all, a cap per sound, the
//! same sound started again within 40 ms merged into the one already playing (a little louder), and when a cap is
//! hit the quietest, least important voice gives way, or the new sound is dropped if it would be that voice.

use super::wav::Clip;

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
}

/// Merge window and steal fade, in seconds.
const MERGE: f32 = 0.040;
const FADE: f32 = 0.020;

pub struct Mixer {
    /// Output samples per second.
    pub rate: u32,
    clips: Vec<Clip>,
    voices: Vec<Voice>,
    /// Output frames rendered so far: the mixer's clock.
    now: u64,
    pub bus_gain: [f32; 4],
    pub master: f32,
    pub muted: bool,
    /// Voices playing at once, all buses together (fading ones aside).
    pub max_voices: usize,
}

impl Mixer {
    pub fn new(rate: u32) -> Mixer {
        Mixer {
            rate,
            clips: Vec::new(),
            voices: Vec::new(),
            now: 0,
            // Starting levels from the design: music -6 dB, ui -3 dB.
            bus_gain: [1.0, db(-3.0), 1.0, db(-6.0)],
            master: 1.0,
            muted: false,
            max_voices: 24,
        }
    }

    pub fn add_clip(&mut self, clip: Clip) -> ClipId {
        self.clips.push(clip);
        ClipId(self.clips.len() as u32 - 1)
    }

    pub fn clip(&self, id: ClipId) -> &Clip {
        &self.clips[id.0 as usize]
    }

    /// Voices playing, not counting ones fading out.
    pub fn playing(&self) -> usize {
        self.voices.iter().filter(|v| v.fading.is_none()).count()
    }

    pub fn playing_key(&self, key: u32) -> usize {
        self.voices.iter().filter(|v| v.fading.is_none() && v.sound.key == key).count()
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
            self.voices.iter_mut().find(|v| v.fading.is_none() && v.sound.key == sound.key && now - v.started <= merge)
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
                .filter(|(_, v)| v.fading.is_none())
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
        self.voices.push(Voice { sound, at: 0.0, started: now, base_gain: sound.gain, fading: None });
        if stolen { Played::Stole } else { Played::Started }
    }

    /// Stop everything at once.
    pub fn stop_all(&mut self) {
        self.voices.clear();
    }

    /// Fill `out`, interleaved with `channels` channels, and advance the clock. Stereo goes to the first two
    /// channels; a mono device gets the two averaged; any further channels stay silent.
    pub fn render(&mut self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        let channels = channels.max(1);
        let frames = out.len() / channels;
        let master = if self.muted { 0.0 } else { self.master };
        for v in &mut self.voices {
            let clip = &self.clips[v.sound.clip.0 as usize];
            let step = clip.rate as f64 / self.rate as f64 * v.sound.speed.max(0.01) as f64;
            let gain = v.sound.gain * self.bus_gain[v.sound.bus as usize] * master;
            let pan = v.sound.pan.clamp(-1.0, 1.0);
            // Constant-power pan.
            let (gl, gr) = (((1.0 - pan) / 2.0).sqrt() * gain, ((1.0 + pan) / 2.0).sqrt() * gain);
            let fade_len = (FADE * self.rate as f32).max(1.0);
            for f in 0..frames {
                let i = v.at as usize;
                if i + 1 >= clip.samples.len() {
                    v.at = clip.samples.len() as f64;
                    break;
                }
                let t = (v.at - i as f64) as f32;
                let mut s = clip.samples[i] * (1.0 - t) + clip.samples[i + 1] * t;
                if let Some(left) = &mut v.fading {
                    if *left == 0 {
                        break;
                    }
                    s *= *left as f32 / fade_len;
                    *left -= 1;
                }
                let o = &mut out[f * channels..(f + 1) * channels];
                if channels == 1 {
                    o[0] += s * (gl + gr) / 2.0;
                } else {
                    o[0] += s * gl;
                    o[1] += s * gr;
                }
                v.at += step;
            }
        }
        let clips = &self.clips;
        self.voices
            .retain(|v| v.fading != Some(0) && (v.at as usize) + 1 < clips[v.sound.clip.0 as usize].samples.len());
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
    fn muted_drops_everything() {
        let mut m = Mixer::new(1000);
        let c = tone(&mut m, 100);
        m.muted = true;
        assert_eq!(m.play(sound(c, 0)), Played::Dropped);
    }
}
