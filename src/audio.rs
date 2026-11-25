//! Music and sound effects, synthesised in code (no audio files): chiptune-style square lead,
//! triangle bass and noise drums, rendered once at startup to 16-bit WAV bytes.

pub const SR: u32 = 44100; // the mixer runs at 44.1 kHz; other rates get a crude resample

fn freq(midi: i32) -> f32 {
    440.0 * 2f32.powf((midi - 69) as f32 / 12.0)
}

/// Deterministic noise (xorshift) so the drums sound the same every run.
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn square(phase: f32, duty: f32) -> f32 {
    if phase.fract() < duty { 1.0 } else { -1.0 }
}
fn triangle(phase: f32) -> f32 {
    4.0 * (phase.fract() - 0.5).abs() - 1.0
}

/// Add one note to `buf`: start/len in seconds.
fn tone(buf: &mut [f32], start: f32, len: f32, midi: i32, amp: f32, kind: u8) {
    let (s0, n) = ((start * SR as f32) as usize, (len * SR as f32) as usize);
    let f = freq(midi);
    for i in 0..n {
        let Some(out) = buf.get_mut(s0 + i) else { break };
        let t = i as f32 / SR as f32;
        let env = (t / 0.004).min(1.0) * (1.0 - i as f32 / n as f32).powf(0.6) * (0.7 + 0.3 * (-t * 8.0).exp());
        let ph = f * t;
        let v = match kind {
            0 => square(ph, 0.25),
            1 => triangle(ph),
            _ => 0.6 * square(ph, 0.5) + 0.4 * triangle(ph * 2.0),
        };
        *out += v * env * amp;
    }
}

fn kick(buf: &mut [f32], start: f32, amp: f32) {
    let s0 = (start * SR as f32) as usize;
    let mut ph = 0.0;
    for i in 0..(0.14 * SR as f32) as usize {
        let Some(out) = buf.get_mut(s0 + i) else { break };
        let t = i as f32 / SR as f32;
        ph += (50.0 + 110.0 * (-t * 30.0).exp()) / SR as f32;
        *out += (ph * std::f32::consts::TAU).sin() * (-t * 22.0).exp() * amp;
    }
}

fn hiss(buf: &mut [f32], start: f32, len: f32, amp: f32, bright: bool, rng: &mut Noise) {
    let s0 = (start * SR as f32) as usize;
    let mut prev = 0.0;
    for i in 0..(len * SR as f32) as usize {
        let Some(out) = buf.get_mut(s0 + i) else { break };
        let t = i as f32 / SR as f32;
        let n = rng.next();
        let v = if bright { n - prev } else { n };
        prev = n;
        *out += v * (-t / len * 5.0).exp() * amp;
    }
}

fn finish(buf: Vec<f32>) -> Vec<f32> {
    buf.into_iter().map(|v| (v * 1.3).tanh() * 0.9).collect()
}

struct Song {
    bpm: f32,
    /// chord roots (midi), one per bar
    roots: &'static [i32],
    /// lead notes per 8th: semitones over the bar's root + 12, or None for a rest
    lead: &'static [Option<i32>],
    snare_on: &'static [usize],
    lead_amp: f32,
}

fn render_song(s: &Song) -> Vec<f32> {
    let step = 60.0 / s.bpm / 4.0; // 16th note
    let bars = s.roots.len();
    let mut buf = vec![0.0; (bars as f32 * 16.0 * step * SR as f32) as usize];
    let mut rng = Noise(0x9e3779b9);
    for (b, &root) in s.roots.iter().enumerate() {
        let t0 = b as f32 * 16.0 * step;
        for i in 0..16 {
            let t = t0 + i as f32 * step;
            // bouncy octave bass on 8ths
            if i % 2 == 0 {
                tone(&mut buf, t, step * 1.8, root - 12 + if i % 4 == 2 { 12 } else { 0 }, 0.28, 1);
            }
            if i % 4 == 0 {
                kick(&mut buf, t, 0.55);
            }
            if s.snare_on.contains(&i) {
                hiss(&mut buf, t, 0.12, 0.22, false, &mut rng);
            }
            hiss(&mut buf, t, 0.03, if i % 2 == 0 { 0.07 } else { 0.04 }, true, &mut rng);
        }
        for i in 0..8 {
            if let Some(Some(n)) = s.lead.get((b * 8 + i) % s.lead.len()) {
                tone(&mut buf, t0 + i as f32 * 2.0 * step, step * 1.9, root + 12 + n, s.lead_amp, 0);
            }
        }
    }
    finish(buf)
}

/// Cheerful lobby / results loop (C – Am – F – G, 124 bpm).
pub fn music_lobby() -> Vec<f32> {
    const N: Option<i32> = None;
    render_song(&Song {
        bpm: 124.0,
        roots: &[48, 45, 41, 43, 48, 45, 41, 43],
        lead: &[
            Some(12), Some(16), Some(19), Some(16), Some(24), N, Some(19), Some(16), // C
            Some(15), Some(19), Some(24), N, Some(19), Some(15), Some(12), N, // Am
            Some(19), Some(24), Some(28), Some(24), Some(19), N, Some(16), Some(19), // F
            Some(17), Some(21), Some(24), Some(21), Some(26), N, Some(24), N, // G
            Some(12), N, Some(16), Some(19), Some(24), Some(19), Some(16), N, // C
            Some(15), Some(19), Some(22), Some(24), N, Some(19), Some(15), N, // Am
            Some(19), N, Some(24), Some(23), Some(24), Some(28), Some(24), Some(19), // F
            Some(17), Some(21), Some(26), N, Some(29), Some(28), Some(26), N, // G
        ],
        snare_on: &[4, 12],
        lead_amp: 0.15,
    })
}

/// Driving loop during questions (Am – F – C – G, 150 bpm), quieter lead so people can think.
pub fn music_quiz() -> Vec<f32> {
    const N: Option<i32> = None;
    render_song(&Song {
        bpm: 150.0,
        roots: &[45, 41, 48, 43],
        lead: &[
            Some(12), N, Some(15), Some(19), N, Some(15), Some(12), N,
            Some(19), N, Some(16), Some(12), N, Some(16), Some(19), N,
            Some(12), N, Some(16), Some(19), N, Some(24), Some(19), N,
            Some(14), N, Some(17), Some(21), N, Some(23), Some(26), N,
        ],
        snare_on: &[4, 12, 14],
        lead_amp: 0.09,
    })
}

#[derive(Clone, Copy, Debug)]
pub enum Sfx {
    Tick,
    Go,
    Blip,
    Correct,
    TimeUp,
    Fanfare,
    Click,
    Whoosh,
}

pub const ALL_SFX: [Sfx; 8] = [Sfx::Tick, Sfx::Go, Sfx::Blip, Sfx::Correct, Sfx::TimeUp, Sfx::Fanfare, Sfx::Click, Sfx::Whoosh];

pub fn sfx(kind: Sfx) -> Vec<f32> {
    let mut b = vec![0.0; SR as usize * 2];
    let len = match kind {
        Sfx::Tick => { tone(&mut b, 0.0, 0.08, 81, 0.5, 2); 0.1 }
        Sfx::Go => { tone(&mut b, 0.0, 0.12, 84, 0.45, 2); tone(&mut b, 0.1, 0.3, 91, 0.45, 2); 0.42 }
        Sfx::Blip => { tone(&mut b, 0.0, 0.05, 76, 0.35, 0); tone(&mut b, 0.04, 0.07, 83, 0.35, 0); 0.12 }
        Sfx::Correct => {
            for (i, n) in [72, 76, 79, 84].iter().enumerate() {
                tone(&mut b, i as f32 * 0.07, 0.25, *n, 0.4, 2);
            }
            0.55
        }
        Sfx::TimeUp => { tone(&mut b, 0.0, 0.22, 52, 0.45, 2); tone(&mut b, 0.2, 0.4, 47, 0.45, 2); 0.62 }
        Sfx::Fanfare => {
            for (i, (n, d)) in [(67, 0.14), (72, 0.14), (76, 0.14), (79, 0.3), (76, 0.14), (79, 0.8)].iter().enumerate() {
                let t = [0.0, 0.15, 0.3, 0.45, 0.78, 0.93][i];
                tone(&mut b, t, *d, *n, 0.4, 2);
                tone(&mut b, t, *d, n - 12, 0.25, 1);
            }
            1.8
        }
        Sfx::Click => { tone(&mut b, 0.0, 0.03, 96, 0.25, 0); 0.04 }
        Sfx::Whoosh => {
            let mut rng = Noise(7);
            let n = (0.35 * SR as f32) as usize;
            let mut lp = 0.0;
            for (i, out) in b.iter_mut().take(n).enumerate() {
                let x = i as f32 / n as f32;
                let k = 0.05 + 0.5 * (x * std::f32::consts::PI).sin();
                lp += (rng.next() - lp) * k;
                *out = lp * (x * std::f32::consts::PI).sin() * 0.5;
            }
            0.36
        }
    };
    b.truncate((len * SR as f32) as usize);
    finish(b)
}

/// 16-bit mono PCM WAV.
pub fn wav(samples: &[f32]) -> Vec<u8> {
    wav_ch(samples, 1)
}

/// 16-bit PCM WAV at 44.1 kHz; `samples` interleaved when `channels` is 2.
pub fn wav_ch(samples: &[f32], channels: u16) -> Vec<u8> {
    let data_len = samples.len() as u32 * 2;
    let mut v = Vec::with_capacity(44 + data_len as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&SR.to_le_bytes());
    v.extend_from_slice(&(SR * 2 * channels as u32).to_le_bytes());
    v.extend_from_slice(&(2 * channels).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        v.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    v
}

/// Longest part of a song that is kept: the mixer holds it as 32-bit stereo (~85 MB for 4 min).
const MAX_SONG_SECS: usize = 240;

/// Decode a song file (mp3, m4a/aac, ogg, flac, wav) into 44.1 kHz stereo WAV bytes for the mixer.
pub fn decode_song(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use symphonia::core::{audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions, probe::Hint};
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|_| "this file type isn't supported (use mp3, m4a, ogg, flac or wav)".to_string())?;
    let mut format = probed.format;
    let track = format.default_track().ok_or("no audio in this file")?;
    let (id, rate) = (track.id, track.codec_params.sample_rate.ok_or("unknown sample rate")?);
    let mut dec = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| "this audio codec isn't supported".to_string())?;
    let max = rate as usize * 2 * MAX_SONG_SECS;
    let mut st: Vec<f32> = Vec::new(); // interleaved stereo
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != id {
            continue;
        }
        let Ok(buf) = dec.decode(&packet) else { continue }; // skip a damaged packet
        let spec = *buf.spec();
        let ch = spec.channels.count().max(1);
        let mut sb = SampleBuffer::<f32>::new(buf.capacity() as u64, spec);
        sb.copy_interleaved_ref(buf);
        for f in sb.samples().chunks(ch) {
            st.push(f[0]);
            st.push(if ch > 1 { f[1] } else { f[0] });
        }
        if st.len() >= max {
            break;
        }
    }
    if st.is_empty() {
        return Err("no sound could be read from this file".into());
    }
    Ok(wav_ch(&resample_stereo(&st, rate, SR), 2))
}

/// Linear resampling of interleaved stereo.
pub fn resample_stereo(st: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return st.to_vec();
    }
    let frames = st.len() / 2;
    let out_frames = (frames as u64 * to as u64 / from as u64) as usize;
    let mut out = Vec::with_capacity(out_frames * 2);
    for i in 0..out_frames {
        let pos = i as f64 * from as f64 / to as f64;
        let (j, k) = (pos as usize, (pos.fract()) as f32);
        let j1 = (j + 1).min(frames - 1);
        for c in 0..2 {
            out.push(st[j * 2 + c] * (1.0 - k) + st[j1 * 2 + c] * k);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_are_audible_and_in_range() {
        for k in ALL_SFX {
            let s = sfx(k);
            assert!(!s.is_empty(), "{k:?}");
            assert!(s.iter().all(|v| v.abs() <= 1.0));
            assert!(s.iter().any(|v| v.abs() > 0.05), "{k:?} is silent");
        }
    }

    #[test]
    fn music_loops_have_whole_bars() {
        let m = music_lobby();
        let bar = 60.0 / 124.0 * 4.0;
        assert!((m.len() as f32 / SR as f32 - 8.0 * bar).abs() < 0.01);
        assert!(m.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
        assert!(!music_quiz().is_empty());
    }

    #[test]
    fn a_song_file_round_trips_through_the_decoder() {
        // 1 s of 22.05 kHz mono tone written as WAV, decoded back as 44.1 kHz stereo
        let mono: Vec<f32> = (0..22050).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let mut bytes = wav(&mono);
        bytes[24..28].copy_from_slice(&22050u32.to_le_bytes()); // sample rate field
        bytes[28..32].copy_from_slice(&44100u32.to_le_bytes()); // byte rate field
        let path = std::env::temp_dir().join("quiz-race-test-song.wav");
        std::fs::write(&path, &bytes).unwrap();
        let out = decode_song(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(u16::from_le_bytes([out[22], out[23]]), 2, "stereo");
        assert_eq!(u32::from_le_bytes(out[24..28].try_into().unwrap()), 44100);
        let frames = (out.len() - 44) / 4;
        assert!((frames as i64 - 44100).abs() <= 2, "{frames} frames");
        assert!(decode_song(std::path::Path::new("Cargo.toml")).is_err());
    }

    #[test]
    fn resampling_keeps_duration() {
        let st = vec![0.25f32; 2 * 48000];
        let out = resample_stereo(&st, 48000, 44100);
        assert_eq!(out.len(), 2 * 44100);
        assert!(out.iter().all(|&v| (v - 0.25).abs() < 1e-6));
    }

    #[test]
    fn wav_header() {
        let w = wav(&[0.0, 1.0, -1.0]);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        assert_eq!(i16::from_le_bytes([w[46], w[47]]), 32767);
    }
}
