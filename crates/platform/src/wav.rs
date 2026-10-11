//! Reads and writes WAV files, the format sound clips and music are stored in for now. Only what packs need: 8- or
//! 16-bit integer PCM, or 4-bit IMA ADPCM (a quarter of the size, for music), any rate, any channel count.
//!
//! A [`Clip`] is decoded whole and mixed down to mono, for short sounds. A [`Track`] keeps its channels (mono or
//! stereo) and, when it is ADPCM, stays compressed: the mixer decodes it a block at a time as it plays, so a long
//! piece of music costs no decoding up front and a quarter of the memory. IMA ADPCM is the public Interactive
//! Multimedia Association format of 1992 as WAV files carry it (format tag 0x11); the code here is written from
//! that description.

/// A sound clip: mono samples from -1 to 1 at `rate` samples per second.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub rate: u32,
    pub samples: Vec<f32>,
}

impl Clip {
    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / self.rate as f32
    }
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// Decode a RIFF WAV file holding 8- or 16-bit PCM or IMA ADPCM, mixed down to mono.
pub fn decode(bytes: &[u8]) -> Result<Clip, String> {
    if u16_at(bytes, fmt_at(bytes)?) == Some(ADPCM) {
        let t = decode_track(bytes)?;
        let ch = t.channels as usize;
        let mut samples = Vec::with_capacity(t.frames);
        let mut block = Vec::new();
        for k in 0..t.blocks() {
            t.block(k, &mut block);
            samples.extend(block.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32));
        }
        samples.truncate(t.frames);
        return Ok(Clip { rate: t.rate, samples });
    }
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a RIFF WAVE file".into());
    }
    let (mut format, mut data) = (None, None);
    let mut at = 12;
    while let (Some(id), Some(len)) = (bytes.get(at..at + 4), u32_at(bytes, at + 4)) {
        let body = at + 8;
        let end = body.checked_add(len as usize).filter(|&e| e <= bytes.len()).ok_or("a chunk runs past the end")?;
        match id {
            b"fmt " => format = Some(body),
            b"data" => data = Some(&bytes[body..end]),
            _ => {}
        }
        // Chunks are padded to an even length.
        at = end + (len as usize & 1);
    }
    let fmt = format.ok_or("no fmt chunk")?;
    let data = data.ok_or("no data chunk")?;
    let short = || "fmt chunk too short".to_string();
    let (kind, channels) = (u16_at(bytes, fmt).ok_or_else(short)?, u16_at(bytes, fmt + 2).ok_or_else(short)?);
    let (rate, bits) = (u32_at(bytes, fmt + 4).ok_or_else(short)?, u16_at(bytes, fmt + 14).ok_or_else(short)?);
    if kind != 1 {
        return Err(format!("format {kind} is not integer PCM"));
    }
    if channels == 0 || rate == 0 {
        return Err("no channels or a zero sample rate".into());
    }
    let one = |b: &[u8]| -> f32 {
        match bits {
            8 => (b[0] as f32 - 128.0) / 128.0,
            _ => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
        }
    };
    if bits != 8 && bits != 16 {
        return Err(format!("{bits}-bit samples; only 8 and 16 are read"));
    }
    let frame = channels as usize * bits as usize / 8;
    let samples = data
        .chunks_exact(frame)
        .map(|f| f.chunks_exact(bits as usize / 8).map(one).sum::<f32>() / channels as f32)
        .collect();
    Ok(Clip { rate, samples })
}

/// IMA ADPCM's format tag.
const ADPCM: u16 = 0x11;

/// Where the `fmt ` chunk's body starts.
fn fmt_at(bytes: &[u8]) -> Result<usize, String> {
    Ok(chunks(bytes)?.iter().find(|c| &c.0 == b"fmt ").ok_or("no fmt chunk")?.1)
}

/// Every chunk: its id and where its body starts and ends.
fn chunks(bytes: &[u8]) -> Result<Vec<([u8; 4], usize, usize)>, String> {
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a RIFF WAVE file".into());
    }
    let mut out = Vec::new();
    let mut at = 12;
    while let (Some(id), Some(len)) = (bytes.get(at..at + 4), u32_at(bytes, at + 4)) {
        let body = at + 8;
        let end = body.checked_add(len as usize).filter(|&e| e <= bytes.len()).ok_or("a chunk runs past the end")?;
        out.push((id.try_into().expect("four bytes"), body, end));
        at = end + (len as usize & 1);
    }
    Ok(out)
}

/// A piece of music (or any long sound): its channels kept, and kept compressed when it is ADPCM.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub rate: u32,
    /// 1 or 2; a file with more is mixed down to mono.
    pub channels: u16,
    /// Length in frames (one sample per channel).
    pub frames: usize,
    data: TrackData,
}

#[derive(Clone, Debug, PartialEq)]
enum TrackData {
    /// Interleaved samples from -1 to 1.
    Pcm(Vec<f32>),
    /// The `data` chunk's blocks, each `align` bytes holding `per_block` frames.
    Adpcm { bytes: Vec<u8>, align: usize, per_block: usize },
}

/// How far the ADPCM step moves for each code, and the 89 step sizes, from the IMA description.
const INDEX_MOVE: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];
const STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107,
    118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
    1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];

/// One ADPCM channel's running state.
#[derive(Clone, Copy, Debug, Default)]
struct Adpcm {
    sample: i32,
    index: i32,
}

impl Adpcm {
    /// Take one 4-bit code, giving the next sample.
    fn decode(&mut self, code: u8) -> i16 {
        let step = STEPS[self.index as usize];
        let mut diff = step >> 3;
        if code & 1 != 0 {
            diff += step >> 2;
        }
        if code & 2 != 0 {
            diff += step >> 1;
        }
        if code & 4 != 0 {
            diff += step;
        }
        self.sample = if code & 8 != 0 { self.sample - diff } else { self.sample + diff }.clamp(-32768, 32767);
        self.index = (self.index + INDEX_MOVE[(code & 7) as usize]).clamp(0, 88);
        self.sample as i16
    }

    /// The code that brings the sample nearest to `s`, taking it as `decode` would.
    fn encode(&mut self, s: i16) -> u8 {
        let step = STEPS[self.index as usize];
        let mut d = s as i32 - self.sample;
        let mut code = 0u8;
        if d < 0 {
            code = 8;
            d = -d;
        }
        let mut part = step;
        for bit in [4u8, 2, 1] {
            if d >= part {
                code |= bit;
                d -= part;
            }
            part >>= 1;
        }
        self.decode(code);
        code
    }
}

impl Track {
    pub fn seconds(&self) -> f32 {
        self.frames as f32 / self.rate as f32
    }

    /// Blocks to decode, one at a time, with [`Track::block`]. A PCM track is one block.
    pub fn blocks(&self) -> usize {
        match &self.data {
            TrackData::Pcm(_) => 1,
            TrackData::Adpcm { bytes, align, .. } => bytes.len() / align,
        }
    }

    /// Frames in each block (the last may stop short of it).
    pub fn per_block(&self) -> usize {
        match &self.data {
            TrackData::Pcm(_) => self.frames.max(1),
            TrackData::Adpcm { per_block, .. } => *per_block,
        }
    }

    /// Decode block `k` into `out`, interleaved, replacing what it held.
    pub fn block(&self, k: usize, out: &mut Vec<f32>) {
        out.clear();
        match &self.data {
            TrackData::Pcm(s) => out.extend_from_slice(s),
            TrackData::Adpcm { bytes, align, per_block } => {
                let ch = self.channels as usize;
                let Some(b) = bytes.get(k * align..(k + 1) * align) else { return };
                out.resize(per_block * ch, 0.0);
                let mut state = [Adpcm::default(); 2];
                for (c, st) in state.iter_mut().enumerate().take(ch) {
                    st.sample = i16::from_le_bytes([b[4 * c], b[4 * c + 1]]) as i32;
                    st.index = (b[4 * c + 2] as i32).clamp(0, 88);
                    out[c] = st.sample as f32 / 32768.0;
                }
                // After the headers, each channel in turn gives 4 bytes: 8 codes, the low nibble first.
                let body = &b[4 * ch..];
                for (g, group) in body.chunks_exact(4 * ch).enumerate() {
                    for (c, part) in group.chunks_exact(4).enumerate() {
                        for (j, byte) in part.iter().enumerate() {
                            for (h, code) in [byte & 15, byte >> 4].into_iter().enumerate() {
                                let frame = 1 + g * 8 + j * 2 + h;
                                if frame < *per_block {
                                    out[frame * ch + c] = state[c].decode(code) as f32 / 32768.0;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Read a WAV file as a [`Track`]: PCM is decoded now, ADPCM kept compressed.
pub fn decode_track(bytes: &[u8]) -> Result<Track, String> {
    let list = chunks(bytes)?;
    let fmt = list.iter().find(|c| &c.0 == b"fmt ").ok_or("no fmt chunk")?.1;
    let &(_, start, end) = list.iter().find(|c| &c.0 == b"data").ok_or("no data chunk")?;
    let short = || "fmt chunk too short".to_string();
    if u16_at(bytes, fmt) != Some(ADPCM) {
        // PCM: decode it per channel, then keep one or two of them.
        let clip_channels = u16_at(bytes, fmt + 2).ok_or_else(short)?;
        let bits = u16_at(bytes, fmt + 14).ok_or_else(short)?;
        let mono = decode(bytes)?;
        if clip_channels != 2 {
            return Ok(Track {
                rate: mono.rate,
                channels: 1,
                frames: mono.samples.len(),
                data: TrackData::Pcm(mono.samples),
            });
        }
        let one = |b: &[u8]| -> f32 {
            match bits {
                8 => (b[0] as f32 - 128.0) / 128.0,
                _ => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
            }
        };
        let samples: Vec<f32> = bytes[start..end].chunks_exact(bits as usize / 8).map(one).collect();
        let frames = samples.len() / 2;
        return Ok(Track { rate: mono.rate, channels: 2, frames, data: TrackData::Pcm(samples) });
    }
    let channels = u16_at(bytes, fmt + 2).ok_or_else(short)?;
    let rate = u32_at(bytes, fmt + 4).ok_or_else(short)?;
    let align = u16_at(bytes, fmt + 12).ok_or_else(short)? as usize;
    let bits = u16_at(bytes, fmt + 14).ok_or_else(short)?;
    if !(1..=2).contains(&channels) || rate == 0 || bits != 4 {
        return Err(format!(
            "ADPCM with {channels} channels at {rate} Hz, {bits} bits; only 4-bit mono or stereo is read"
        ));
    }
    let ch = channels as usize;
    if align <= 4 * ch || !(align - 4 * ch).is_multiple_of(4 * ch) {
        return Err(format!("ADPCM blocks of {align} bytes don't fit {channels} channels"));
    }
    let per_block = (align - 4 * ch) * 2 / ch + 1;
    let data = bytes[start..end].to_vec();
    let blocks = data.len() / align;
    // The `fact` chunk gives the true length; without it every block counts in full.
    let frames = list
        .iter()
        .find(|c| &c.0 == b"fact")
        .and_then(|c| u32_at(bytes, c.1))
        .map_or(blocks * per_block, |n| (n as usize).min(blocks * per_block));
    Ok(Track { rate, channels, frames, data: TrackData::Adpcm { bytes: data, align, per_block } })
}

/// Encode 16-bit samples, interleaved when `channels` is 2, as an IMA ADPCM WAV file of `align`-byte blocks (1024
/// is usual at 22 or 44 kHz). For tools and tests; the engine only reads.
pub fn encode_adpcm(rate: u32, channels: u16, samples: &[i16], align: usize) -> Vec<u8> {
    let ch = channels.clamp(1, 2) as usize;
    let per_block = (align - 4 * ch) * 2 / ch + 1;
    let frames = samples.len() / ch;
    let mut data = Vec::new();
    let mut state = [Adpcm::default(); 2];
    let at = |f: usize, c: usize| samples.get(f * ch + c).copied().unwrap_or(0);
    let mut f0 = 0;
    while f0 < frames {
        for (c, st) in state.iter_mut().enumerate().take(ch) {
            st.sample = at(f0, c) as i32;
            data.extend_from_slice(&(st.sample as i16).to_le_bytes());
            data.extend_from_slice(&[st.index as u8, 0]);
        }
        let mut f = f0 + 1;
        while f < f0 + per_block {
            for (c, st) in state.iter_mut().enumerate().take(ch) {
                for j in 0..4 {
                    let lo = st.encode(at(f + j * 2, c));
                    let hi = st.encode(at(f + j * 2 + 1, c));
                    data.push(lo | hi << 4);
                }
            }
            f += 8;
        }
        f0 += per_block;
    }
    let mut b = Vec::with_capacity(60 + data.len());
    let byte_rate = rate as usize * align / per_block;
    b.extend_from_slice(b"RIFF");
    // The size after this field: "WAVE", then the fmt (8 + 20), fact (8 + 4) and data chunks.
    b.extend_from_slice(&(4 + 28 + 12 + 8 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&20u32.to_le_bytes());
    for v in [ADPCM, ch as u16] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(byte_rate as u32).to_le_bytes());
    for v in [align as u16, 4, 2, per_block as u16] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(b"fact");
    b.extend_from_slice(&4u32.to_le_bytes());
    b.extend_from_slice(&(frames as u32).to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    b
}

/// Encode 16-bit mono samples as a WAV file.
pub fn encode(rate: u32, samples: &[i16]) -> Vec<u8> {
    let data = samples.len() as u32 * 2;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let samples = [0i16, 16384, -16384, 32767, -32768];
        let clip = decode(&encode(22050, &samples)).unwrap();
        assert_eq!(clip.rate, 22050);
        assert_eq!(clip.samples, vec![0.0, 0.5, -0.5, 32767.0 / 32768.0, -1.0]);
    }

    #[test]
    fn adpcm_round_trip_stays_close_and_keeps_its_length() {
        // A sweep, stereo, with the right channel the left's opposite.
        let frames = 5000;
        let mut samples = Vec::new();
        for i in 0..frames {
            let s = ((i as f32 * (0.02 + i as f32 * 0.00001)).sin() * 20000.0) as i16;
            samples.extend([s, -s]);
        }
        let bytes = encode_adpcm(22050, 2, &samples, 1024);
        let t = decode_track(&bytes).unwrap();
        assert_eq!((t.rate, t.channels, t.frames), (22050, 2, frames));
        assert!(bytes.len() < samples.len() * 2 / 3, "ADPCM is about a quarter the size: {}", bytes.len());
        let mut out = Vec::new();
        let mut decoded = Vec::new();
        for k in 0..t.blocks() {
            t.block(k, &mut out);
            decoded.extend_from_slice(&out);
        }
        decoded.truncate(frames * 2);
        let worst = decoded.iter().zip(&samples).map(|(d, &s)| (d * 32768.0 - s as f32).abs()).fold(0.0f32, f32::max);
        assert!(worst < 2500.0, "worst error {worst}");
        // The mono clip is the two channels averaged: silence here.
        let clip = decode(&bytes).unwrap();
        assert_eq!(clip.samples.len(), frames);
        assert!(clip.samples.iter().all(|s| s.abs() < 0.08));
    }

    #[test]
    fn a_pcm_track_keeps_two_channels() {
        let mut b = encode(8000, &[100, -100, 200, -200]);
        // Say it is stereo: two frames of two samples.
        b[22] = 2;
        let t = decode_track(&b).unwrap();
        assert_eq!((t.channels, t.frames), (2, 2));
        let mut out = Vec::new();
        t.block(0, &mut out);
        assert_eq!(out.len(), 4);
        assert!(out[0] > 0.0 && out[1] < 0.0);
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        assert!(decode(b"not a wav").is_err());
        let mut float = encode(8000, &[0, 0]);
        float[20] = 3;
        assert!(decode(&float).unwrap_err().contains("not integer PCM"));
        let mut cut = encode(8000, &[0, 0, 0]);
        cut.truncate(46);
        assert!(decode(&cut).is_err());
    }
}
