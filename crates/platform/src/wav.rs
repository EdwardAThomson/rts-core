//! Reads and writes plain PCM WAV files, the format sound clips are stored in for now. Only what packs need:
//! 8- or 16-bit integer samples, any rate, any channel count (mixed down to mono on reading).

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

/// Decode a RIFF WAV file holding 8- or 16-bit PCM.
pub fn decode(bytes: &[u8]) -> Result<Clip, String> {
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
