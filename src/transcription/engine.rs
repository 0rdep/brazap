//! Audio decoding and transcribe.cpp, compiled with the `transcription` feature.
//!
//! Memory: transcribe.cpp memory-maps a model's weights from its GGUF file on
//! the CPU (and Metal on Apple silicon), so they live in the OS page cache,
//! which the system can reclaim, instead of the heap ([`ModelOptions::no_mmap`]
//! stays `false`). A session keeps scheduler buffers sized by the longest audio
//! it has run for its whole lifetime, so every transcription opens a fresh
//! session and drops it afterwards; idle, only the mapping stays until the
//! worker unloads the model.
use anyhow::{Context, Result, ensure};
use std::io::Cursor;
use symphonia::core::{
    codecs::CODEC_TYPE_OPUS, errors::Error, formats::FormatOptions, io::MediaSourceStream,
    meta::MetadataOptions, probe::Hint,
};
use transcribe_cpp::{
    Backend, DeviceType, ModelOptions, RunOptions, SessionOptions, TimestampKind,
};

use super::{MAX_SECONDS, SAMPLE_RATE, SpeechModel};

pub(super) struct Audio {
    pub samples: Vec<f32>,
    pub partial: bool,
}

/// The x86-64 build targets x86-64-v3 (see .cargo/config.toml).
pub(super) fn unsupported() -> Option<&'static str> {
    #[cfg(target_arch = "x86_64")]
    if !(std::arch::is_x86_feature_detected!("avx2")
        && std::arch::is_x86_feature_detected!("bmi2")
        && std::arch::is_x86_feature_detected!("fma")
        && std::arch::is_x86_feature_detected!("f16c"))
    {
        return Some("This processor doesn't support the instructions voice transcription needs");
    }
    None
}

/// WhatsApp's Ogg (or WebM) Opus voice notes as 16 kHz mono samples.
/// Mirrors playback/voice.rs: OpusHead gain, pre-skip and final granule.
pub(super) fn decode(bytes: &[u8]) -> Result<Audio> {
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return decode_wav(bytes);
    }
    ensure!(
        bytes.starts_with(b"OggS") || bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
        "Not an Opus voice message"
    );
    let ogg = bytes.starts_with(b"OggS");
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut demux = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            stream,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )?
        .format;
    let track = demux
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec == CODEC_TYPE_OPUS)
        .context("This voice message has no Opus track")?;
    let id = track.id;
    let head = track
        .codec_params
        .extra_data
        .as_deref()
        .context("Missing Opus header")?;
    ensure!(
        head.len() >= 19 && &head[..8] == b"OpusHead",
        "Invalid Opus voice header"
    );
    let channels = head[9] as usize;
    ensure!(
        head[18] == 0 && (1..=2).contains(&channels),
        "Unsupported voice channel layout"
    );
    // Opus headers count samples at 48 kHz; this decoder runs at 16 kHz.
    let pre_skip = u16::from_le_bytes([head[10], head[11]]) as usize;
    let mut skip = pre_skip / 3;
    let gain = 10f32.powf(i16::from_le_bytes([head[16], head[17]]) as f32 / 256.0 / 20.0);
    let keep = ogg
        .then_some(track.codec_params.n_frames)
        .flatten()
        .map(|n| n.saturating_sub(pre_skip as u64) as usize / 3);
    let mut decoder = opuscule::Decoder::new(
        opuscule::SampleRate::Hz16000,
        if channels == 1 {
            opuscule::Channels::Mono
        } else {
            opuscule::Channels::Stereo
        },
    );
    let limit = MAX_SECONDS * SAMPLE_RATE;
    let mut buffer = vec![0f32; 5760 * channels];
    let mut samples = Vec::with_capacity(SAMPLE_RATE * 60);
    let mut partial = false;
    loop {
        let packet = match demux.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != id {
            continue;
        }
        let frames = decoder
            .decode(Some(&packet.data), &mut buffer, false)
            .map_err(|e| anyhow::anyhow!("Opus decode failed: {e}"))?;
        let start = skip.min(frames);
        skip -= start;
        for frame in buffer[start * channels..frames * channels].chunks_exact(channels) {
            let mono = frame.iter().sum::<f32>() / channels as f32;
            samples.push((mono * gain).clamp(-1.0, 1.0));
        }
        if samples.len() >= limit {
            partial = true;
            break;
        }
    }
    if let Some(keep) = keep
        && !partial
    {
        samples.truncate(keep);
    }
    samples.truncate(limit);
    ensure!(!samples.is_empty(), "Voice message contains no audio");
    Ok(Audio { samples, partial })
}

/// 16-bit PCM WAV (audio files and demo voice notes), resampled to 16 kHz mono.
fn decode_wav(bytes: &[u8]) -> Result<Audio> {
    let mut offset = 12;
    let mut format = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
        let body = offset + 8;
        let end = body.saturating_add(size).min(bytes.len());
        if id == b"fmt " && end - body >= 16 {
            let field = |at: usize| u16::from_le_bytes([bytes[body + at], bytes[body + at + 1]]);
            let rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into()?) as usize;
            format = Some((field(0), field(2) as usize, rate, field(14)));
        } else if id == b"data" {
            let (kind, channels, rate, bits) = format.context("WAV data before its format")?;
            ensure!(
                matches!(kind, 1 | 0xfffe) && bits == 16 && channels > 0 && rate > 0,
                "Only 16-bit PCM WAV audio can be transcribed"
            );
            let mono: Vec<f32> = bytes[body..end]
                .chunks_exact(2 * channels)
                .map(|frame| {
                    frame
                        .chunks_exact(2)
                        .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0)
                        .sum::<f32>()
                        / channels as f32
                })
                .collect();
            let length = mono.len() * SAMPLE_RATE / rate;
            let limit = MAX_SECONDS * SAMPLE_RATE;
            let samples: Vec<f32> = (0..length.min(limit))
                .map(|i| {
                    let position = i as f64 * rate as f64 / SAMPLE_RATE as f64;
                    let index = position as usize;
                    let next = mono.get(index + 1).copied().unwrap_or(mono[index]);
                    let fraction = (position - index as f64) as f32;
                    mono[index] * (1.0 - fraction) + next * fraction
                })
                .collect();
            ensure!(!samples.is_empty(), "Voice message contains no audio");
            return Ok(Audio {
                samples,
                partial: length > limit,
            });
        }
        offset = body + size + (size & 1);
    }
    anyhow::bail!("WAV audio has no data")
}

/// Parakeet needs at least this much audio; shorter clips are padded with silence.
const PARAKEET_MIN_SAMPLES: usize = SAMPLE_RATE * 3 / 2;
/// Parakeet encodes a whole run at once, and its activations grow with the
/// audio (measured on Windows: 1.0 GB private memory at peak for 45 s, 2.9 GB
/// for 9 minutes). Longer voice messages are transcribed in pieces of at most
/// this length. Whisper already works through 30 s windows.
const PARAKEET_CHUNK_SAMPLES: usize = SAMPLE_RATE * 30;
/// The shortest piece [`pieces`] cuts.
const FRAME_MIN: usize = SAMPLE_RATE;

/// Splits `samples` into pieces of at most `max` samples, each cut at the
/// quietest 100 ms of its last third so words are rarely split.
fn pieces(samples: &[f32], max: usize) -> Vec<&[f32]> {
    const FRAME: usize = SAMPLE_RATE / 10;
    let mut pieces = Vec::new();
    let mut rest = samples;
    while rest.len() > max {
        let mut cut = max;
        let mut quietest = f32::INFINITY;
        let mut start = max * 2 / 3;
        while start + FRAME <= max {
            let energy: f32 = rest[start..start + FRAME].iter().map(|s| s * s).sum();
            if energy < quietest {
                quietest = energy;
                cut = start + FRAME / 2;
            }
            start += FRAME / 2;
        }
        let (piece, tail) = rest.split_at(cut);
        pieces.push(piece);
        rest = tail;
    }
    pieces.push(rest);
    pieces
}

/// CPU threads for one transcription: about one per physical core (half the
/// logical processors on SMT machines), at most 8. On a Ryzen 7 9800X3D (8
/// cores, 16 threads), Whisper large-v3 turbo took 13.7 s for 45 s of audio
/// with 4 threads and 10.4 s with 6 or 8; 12 threads reached 8.3 s, but only
/// by also taking the hyper-threads the window and the rest of the system use
/// while a message is transcribed in the background. 4-core CPUs get 2 to 4.
pub(super) fn threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).clamp(1, 8))
        .unwrap_or(2) as i32
}

/// A model file transcribe.cpp could not read, which downloads again; other
/// load errors (memory, an incompatible library) leave the file alone.
#[derive(Debug)]
pub(super) struct Damaged;
impl std::fmt::Display for Damaged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The speech model file is damaged")
    }
}

pub(super) fn damaged(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Damaged>().is_some()
}

pub(super) struct Model {
    model: transcribe_cpp::Model,
    options: SessionOptions,
    parakeet: bool,
    /// The longest audio one run accepts, in samples; longer audio is split.
    max_samples: Option<usize>,
}

impl Model {
    pub fn load(path: &std::path::Path, expected: &SpeechModel) -> Result<Self> {
        procwatch::alloc::tag_thread("transcription");
        static LOGGING: std::sync::Once = std::sync::Once::new();
        LOGGING.call_once(transcribe_cpp::init_logging);
        // No GPU backend is compiled in except Metal on macOS: `Auto` is the
        // CPU on Windows and Linux. The weights stay memory-mapped.
        let model = transcribe_cpp::Model::load_with(
            path,
            &ModelOptions {
                backend: Backend::Auto,
                device: None,
                no_mmap: false,
            },
        )
        .map_err(|e| {
            let damaged = matches!(
                e,
                transcribe_cpp::Error::ModelLoad(_) | transcribe_cpp::Error::ModelFileNotFound(_)
            );
            let error = anyhow::anyhow!("transcribe.cpp: {e}");
            if damaged {
                error.context(Damaged)
            } else {
                error
            }
        })?;
        let (architecture, variant) = (model.arch(), model.variant());
        if architecture != expected.architecture || variant != expected.variant {
            return Err(anyhow::anyhow!(
                "The model file contains {architecture}/{variant}, expected {}/{}",
                expected.architecture,
                expected.variant
            )
            .context(Damaged));
        }
        let device = model.device().map_err(|e| anyhow::anyhow!("{e}"))?;
        let capabilities = model.capabilities().map_err(|e| anyhow::anyhow!("{e}"))?;
        // On a GPU only the per-token decoder runs on the CPU, where extra
        // spinning workers only slow it down (Hex's `session_options`).
        let n_threads = match device.device_type {
            DeviceType::Gpu | DeviceType::Igpu => 1,
            _ => threads(),
        };
        log::info!(
            "Loaded {} on {} ({}) with {n_threads} threads",
            expected.title,
            device.description,
            device.kind
        );
        Ok(Self {
            model,
            options: SessionOptions {
                n_threads,
                ..Default::default()
            },
            parakeet: architecture == "parakeet",
            max_samples: usize::try_from(capabilities.max_audio_ms)
                .ok()
                .filter(|ms| *ms > 0)
                .map(|ms| ms * SAMPLE_RATE / 1000)
                .into_iter()
                .chain((architecture == "parakeet").then_some(PARAKEET_CHUNK_SAMPLES))
                .min(),
        })
    }

    /// One run on a fresh session, dropped before returning so its
    /// input-sized buffers don't outlive this message.
    fn run(&self, samples: &[f32], language: Option<&str>) -> Result<transcribe_cpp::Transcript> {
        let mut session = self
            .model
            .session_with(&self.options)
            .map_err(|e| anyhow::anyhow!("Could not start transcription: {e}"))?;
        let options = RunOptions {
            // Whisper's long-form loop follows its segment timestamps from
            // window to window; without them a dense 30 s window often ends
            // early and the rest of it is lost. Parakeet needs none.
            timestamps: if self.parakeet {
                TimestampKind::None
            } else {
                TimestampKind::Segment
            },
            language: language.map(str::to_owned),
            ..Default::default()
        };
        let mut transcript = String::new();
        let mut detected = None;
        for chunk in pieces(
            samples,
            self.max_samples.unwrap_or(samples.len()).max(FRAME_MIN),
        ) {
            let padded;
            let mut chunk = chunk;
            if self.parakeet && chunk.len() < PARAKEET_MIN_SAMPLES {
                let mut longer = chunk.to_vec();
                longer.resize(PARAKEET_MIN_SAMPLES, 0.0);
                padded = longer;
                chunk = &padded;
            }
            let result = match session.run(chunk, &options) {
                Ok(result) => result,
                // transcribe.cpp stops a decoder that starts looping and
                // returns the text before the loop with this error.
                Err(transcribe_cpp::Error::OutputRepetition {
                    partial: Some(partial),
                    ..
                }) => *partial,
                Err(e) => anyhow::bail!("Transcription failed: {e}"),
            };
            let text = result.text.trim();
            if !text.is_empty() {
                if !transcript.is_empty() {
                    transcript.push(' ');
                }
                transcript.push_str(text);
            }
            detected = detected.or(result.language);
        }
        drop(session);
        Ok(transcribe_cpp::Transcript {
            text: transcript,
            language: detected,
            ..Default::default()
        })
    }

    /// Returns the text and its language. Whisper detects the language unless
    /// it is told one. Its smaller models often label Portuguese and Spanish
    /// as Galician; when the app language is one of those and Whisper heard
    /// Galician, the message is transcribed again in the app language.
    /// Parakeet always detects the language itself and reports none.
    pub fn transcribe(
        &self,
        samples: &[f32],
        preferred: &str,
        expected: &SpeechModel,
    ) -> Result<(String, String)> {
        procwatch::alloc::tag_thread("transcription");
        if !expected.takes_language_hint() {
            let result = self.run(samples, None)?;
            return Ok((result.text, result.language.unwrap_or_default()));
        }
        let result = self.run(samples, None)?;
        let detected = result.language.clone().unwrap_or_default();
        if detected == "gl" && matches!(preferred, "pt" | "es") {
            let again = self.run(samples, Some(preferred))?;
            return Ok((again.text, preferred.to_owned()));
        }
        Ok((result.text, detected))
    }
}
