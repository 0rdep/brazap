//! Bounded microphone capture and streaming Ogg Opus encoding. The capture
//! callback only downmixes and queues small blocks; encoding runs off the UI.
use std::{
    collections::VecDeque,
    io::{BufWriter, Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use cpal::{
    ErrorKind, SampleFormat, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use ogg::writing::{PacketWriteEndInfo, PacketWriter};

const RATE: u32 = 48_000;
const FRAME: usize = 960;
const MAX_SAMPLES: u64 = RATE as u64 * 600;

pub struct Recorded {
    pub path: tempfile::TempPath,
    pub duration_seconds: u32,
    pub waveform: Vec<u8>,
}

#[derive(Clone, Default, PartialEq)]
pub struct Progress {
    pub started: bool,
    pub seconds: f64,
    pub levels: VecDeque<u8>,
}

pub struct Preview {
    pub path: tempfile::TempPath,
    pub waveform: Vec<u8>,
}

pub enum CaptureEvent {
    Paused(Preview),
    Resumed,
    Finished(Result<Recorded, String>),
}

#[derive(Clone, Copy)]
enum Control {
    Pause,
    Resume,
    Finish,
    Cancel,
}

pub struct Recorder {
    commands: mpsc::Sender<Control>,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<Progress>>,
    events: mpsc::Receiver<CaptureEvent>,
}

impl Recorder {
    pub fn start(demo: bool) -> Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(Progress::default()));
        let (commands, receiver) = mpsc::channel();
        let (send, events) = mpsc::channel();
        let (worker_cancel, worker_progress) = (cancel.clone(), progress.clone());
        std::thread::Builder::new()
            .name("brazap-microphone".into())
            .spawn(move || {
                procwatch::alloc::tag_thread("voice recording");
                let result = capture(demo, &worker_cancel, &worker_progress, receiver, &send);
                if worker_cancel.load(Ordering::Acquire) {
                    return;
                }
                let value = result.map_err(|error| {
                    log::warn!("Voice recording failed: {error:#}");
                    if error.to_string() == "Recording is too short" {
                        "Recording is too short. Record at least half a second.".into()
                    } else {
                        "Could not record audio. Check your microphone and its permissions.".into()
                    }
                });
                let _ = send.send(CaptureEvent::Finished(value));
            })?;
        Ok(Self {
            commands,
            cancel,
            progress,
            events,
        })
    }

    pub fn progress(&self) -> Progress {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn pause(&self) {
        let _ = self.commands.send(Control::Pause);
    }
    pub fn resume(&self) {
        let _ = self.commands.send(Control::Resume);
    }
    pub fn finish(&self) {
        let _ = self.commands.send(Control::Finish);
    }

    pub fn try_event(&self) -> Option<CaptureEvent> {
        match self.events.try_recv() {
            Ok(event) => Some(event),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(CaptureEvent::Finished(Err(
                "Could not finish the voice recording.".into(),
            ))),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        let _ = self.commands.send(Control::Cancel);
    }
}

fn input<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    send: mpsc::SyncSender<Vec<f32>>,
    failed: Arc<AtomicBool>,
    accepting: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels);
    let callback_failed = failed.clone();
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            if !accepting.load(Ordering::Acquire) {
                return;
            }
            let mono = data
                .chunks_exact(channels)
                .map(|frame| {
                    frame.iter().map(|&s| s.to_sample::<f32>()).sum::<f32>() / channels as f32
                })
                .collect();
            if send.try_send(mono).is_err() {
                callback_failed.store(true, Ordering::Release);
            }
        },
        move |error| match error.kind() {
            // CPAL keeps the stream active for these notifications. In
            // particular, WASAPI can report a discontinuity while capture
            // starts; the next packet still contains usable microphone data.
            ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied => {
                log::debug!("Microphone stream continuing: {error}");
            }
            _ => {
                log::warn!("Microphone stream stopped: {error}");
                failed.store(true, Ordering::Release);
            }
        },
        None,
    )?)
}

fn capture(
    demo: bool,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
    commands: mpsc::Receiver<Control>,
    events: &mpsc::Sender<CaptureEvent>,
) -> Result<Recorded> {
    let (send, receive) = mpsc::sync_channel(8);
    let failed = Arc::new(AtomicBool::new(false));
    let accepting = Arc::new(AtomicBool::new(true));
    let (stream, rate) = if demo {
        (None, RATE)
    } else {
        let device = cpal::default_host()
            .default_input_device()
            .context("No microphone is available")?;
        let default = device.default_input_config()?;
        let selected = device
            .supported_input_configs()?
            .find(|c| {
                c.sample_format() == default.sample_format()
                    && c.min_sample_rate() <= RATE
                    && c.max_sample_rate() >= RATE
                    && c.channels() == default.channels()
            })
            .map(|c| c.with_sample_rate(RATE))
            .unwrap_or(default);
        let rate = selected.sample_rate();
        let config = selected.config();
        let stream = match selected.sample_format() {
            SampleFormat::F32 => {
                input::<f32>(&device, config, send, failed.clone(), accepting.clone())?
            }
            SampleFormat::F64 => {
                input::<f64>(&device, config, send, failed.clone(), accepting.clone())?
            }
            SampleFormat::I16 => {
                input::<i16>(&device, config, send, failed.clone(), accepting.clone())?
            }
            SampleFormat::I32 => {
                input::<i32>(&device, config, send, failed.clone(), accepting.clone())?
            }
            SampleFormat::U16 => {
                input::<u16>(&device, config, send, failed.clone(), accepting.clone())?
            }
            SampleFormat::I8 => {
                input::<i8>(&device, config, send, failed.clone(), accepting.clone())?
            }
            SampleFormat::U8 => {
                input::<u8>(&device, config, send, failed.clone(), accepting.clone())?
            }
            SampleFormat::U32 => {
                input::<u32>(&device, config, send, failed.clone(), accepting.clone())?
            }
            _ => bail!("Unsupported microphone sample format"),
        };
        stream.play()?;
        (Some(stream), rate)
    };
    let mut encoder = VoiceEncoder::new()?;
    progress.lock().unwrap_or_else(|e| e.into_inner()).started = true;
    let mut resampler = Resampler::new(rate);
    let mut paused = false;
    loop {
        ensure!(!cancel.load(Ordering::Acquire), "Recording cancelled");
        ensure!(
            !failed.load(Ordering::Acquire),
            "Microphone capture interrupted"
        );
        let command = if paused {
            Some(commands.recv().context("Recorder stopped")?)
        } else {
            match commands.try_recv() {
                Ok(command) => Some(command),
                Err(mpsc::TryRecvError::Empty) if encoder.samples >= MAX_SAMPLES => {
                    Some(Control::Pause)
                }
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => bail!("Recorder stopped"),
            }
        };
        match command {
            Some(Control::Cancel) => bail!("Recording cancelled"),
            Some(Control::Pause | Control::Finish) => {
                // Stop queuing first, then suspend the device. Drain everything
                // already captured so neither pause nor finish clips the tail.
                accepting.store(false, Ordering::Release);
                if let Some(stream) = &stream {
                    stream.pause()?;
                }
                for block in receive.try_iter() {
                    for sample in block {
                        resampler.push(sample, &mut encoder, progress)?;
                    }
                }
                progress.lock().unwrap_or_else(|e| e.into_inner()).seconds =
                    encoder.samples as f64 / f64::from(RATE);
                if matches!(command, Some(Control::Finish)) {
                    break;
                }
                paused = true;
                events
                    .send(CaptureEvent::Paused(encoder.preview()?))
                    .context("Recorder closed")?;
            }
            Some(Control::Resume) if paused && encoder.samples < MAX_SAMPLES => {
                if let Some(stream) = &stream {
                    stream.play()?;
                }
                accepting.store(true, Ordering::Release);
                paused = false;
                events
                    .send(CaptureEvent::Resumed)
                    .context("Recorder closed")?;
            }
            Some(Control::Resume) => {}
            None if demo => {
                std::thread::sleep(Duration::from_millis(20));
                for _ in 0..FRAME {
                    let t = encoder.samples as f32 / RATE as f32;
                    let amplitude = 0.03 + 0.25 * (t * 2.7).sin().abs();
                    encoder.push(
                        (t * 220.0 * std::f32::consts::TAU).sin() * amplitude,
                        progress,
                    )?;
                }
            }
            None => {
                if let Ok(block) = receive.recv_timeout(Duration::from_millis(20)) {
                    for sample in block {
                        resampler.push(sample, &mut encoder, progress)?;
                    }
                }
            }
        }
    }
    drop(stream);
    ensure!(!cancel.load(Ordering::Acquire), "Recording cancelled");
    ensure!(
        !failed.load(Ordering::Acquire),
        "Microphone capture interrupted"
    );
    encoder.finish()
}

/// Linear interpolation is only needed if a microphone cannot provide 48 kHz.
struct Resampler {
    step: f64,
    next: f64,
    index: u64,
    previous: f32,
}
impl Resampler {
    fn new(rate: u32) -> Self {
        Self {
            step: f64::from(rate) / f64::from(RATE),
            next: 0.0,
            index: 0,
            previous: 0.0,
        }
    }
    fn push(
        &mut self,
        sample: f32,
        encoder: &mut VoiceEncoder,
        progress: &Mutex<Progress>,
    ) -> Result<()> {
        let sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        while self.next <= self.index as f64 {
            let fraction = (self.next - (self.index as f64 - 1.0)).clamp(0.0, 1.0) as f32;
            encoder.push(
                self.previous + (sample - self.previous) * fraction,
                progress,
            )?;
            self.next += self.step;
        }
        self.previous = sample;
        self.index += 1;
        Ok(())
    }
}

struct VoiceEncoder {
    encoder: opus::Encoder,
    writer: PacketWriter<'static, Vec<u8>>,
    pending: Option<(Vec<u8>, u64)>,
    frame: Vec<f32>,
    peaks: Vec<u8>,
    samples: u64,
    encoded: u64,
    pre_skip: u16,
    // PCM is spooled to a temporary file for lossless previews, never kept as
    // a full recording in RAM. Sending still uses the streaming Opus encoder.
    pcm: BufWriter<tempfile::NamedTempFile>,
}

impl VoiceEncoder {
    fn new() -> Result<Self> {
        let mut encoder = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip)?;
        encoder.set_bitrate(opus::Bitrate::Bits(24_000))?;
        encoder.set_vbr(true)?;
        let pre_skip = u16::try_from(encoder.get_lookahead()?)?;
        let mut writer = PacketWriter::new(Vec::new());
        let mut header = b"OpusHead\x01\x01".to_vec();
        header.extend(pre_skip.to_le_bytes());
        header.extend(RATE.to_le_bytes());
        header.extend([0, 0, 0]); // gain 0 dB, channel mapping family 0
        writer.write_packet(header, 1, PacketWriteEndInfo::EndPage, 0)?;
        let mut tags = b"OpusTags".to_vec();
        tags.extend(6u32.to_le_bytes());
        tags.extend(b"Brazap");
        tags.extend(0u32.to_le_bytes());
        writer.write_packet(tags, 1, PacketWriteEndInfo::EndPage, 0)?;
        Ok(Self {
            encoder,
            writer,
            pending: None,
            frame: Vec::with_capacity(FRAME),
            peaks: Vec::new(),
            samples: 0,
            encoded: 0,
            pre_skip,
            pcm: BufWriter::with_capacity(
                64 * 1024,
                tempfile::Builder::new()
                    .prefix("brazap-capture-")
                    .suffix(".pcm")
                    .tempfile()?,
            ),
        })
    }

    fn push(&mut self, sample: f32, progress: &Mutex<Progress>) -> Result<()> {
        if self.samples >= MAX_SAMPLES {
            return Ok(());
        }
        self.frame.push(sample);
        self.samples += 1;
        if self.frame.len() == FRAME {
            let mut bytes = [0; FRAME * 2];
            pcm_bytes(&self.frame, &mut bytes);
            self.pcm.write_all(&bytes)?;
            let peak = (self
                .frame
                .iter()
                .map(|n| n.abs())
                .fold(0.0, f32::max)
                .sqrt()
                * 100.0)
                .min(100.0) as u8;
            self.peaks.push(peak);
            self.encode_frame()?;
            let mut view = progress.lock().unwrap_or_else(|e| e.into_inner());
            view.seconds = self.samples as f64 / f64::from(RATE);
            view.levels.push_back(peak);
            if view.levels.len() > 48 {
                view.levels.pop_front();
            }
        }
        Ok(())
    }

    fn encode_frame(&mut self) -> Result<()> {
        self.frame.resize(FRAME, 0.0);
        let mut packet = vec![0; 4000];
        let size = self.encoder.encode_float(&self.frame, &mut packet)?;
        packet.truncate(size);
        self.frame.clear();
        self.encoded += FRAME as u64;
        if let Some((previous, end)) = self.pending.replace((packet, self.encoded)) {
            self.writer
                .write_packet(previous, 1, PacketWriteEndInfo::NormalPacket, end)?;
        }
        Ok(())
    }

    fn waveform(&self) -> Vec<u8> {
        if self.peaks.is_empty() {
            return vec![0; 64];
        }
        let mut waveform = Vec::with_capacity(64);
        for i in 0..64 {
            let start = i * self.peaks.len() / 64;
            let stop = ((i + 1) * self.peaks.len() / 64)
                .max(start + 1)
                .min(self.peaks.len());
            waveform.push(
                self.peaks[start.min(self.peaks.len() - 1)..stop]
                    .iter()
                    .copied()
                    .max()
                    .unwrap_or(0),
            );
        }
        waveform
    }

    fn preview(&mut self) -> Result<Preview> {
        self.pcm.flush()?;
        let mut file = tempfile::Builder::new()
            .prefix("brazap-preview-")
            .suffix(".wav")
            .tempfile()?;
        let bytes = (self.samples * 2) as u32;
        file.write_all(b"RIFF")?;
        file.write_all(&(bytes + 36).to_le_bytes())?;
        file.write_all(b"WAVEfmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&RATE.to_le_bytes())?;
        file.write_all(&(RATE * 2).to_le_bytes())?;
        file.write_all(&2u16.to_le_bytes())?;
        file.write_all(&16u16.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&bytes.to_le_bytes())?;
        let source = self.pcm.get_ref().reopen()?;
        std::io::copy(
            &mut source.take((self.samples - self.frame.len() as u64) * 2),
            &mut file,
        )?;
        let mut tail = vec![0; self.frame.len() * 2];
        pcm_bytes(&self.frame, &mut tail);
        file.write_all(&tail)?;
        file.flush()?;
        Ok(Preview {
            path: file.into_temp_path(),
            waveform: self.waveform(),
        })
    }

    fn finish(mut self) -> Result<Recorded> {
        ensure!(
            self.samples >= u64::from(RATE) / 2,
            "Recording is too short"
        );
        let end = self.samples + u64::from(self.pre_skip);
        while self.encoded < end {
            self.encode_frame()?;
        }
        if let Some((packet, _)) = self.pending.take() {
            self.writer
                .write_packet(packet, 1, PacketWriteEndInfo::EndStream, end)?;
        }
        let waveform = self.waveform();
        let mut file = tempfile::Builder::new()
            .prefix("brazap-recording-")
            .suffix(".ogg")
            .tempfile()?;
        file.write_all(&self.writer.into_inner())?;
        file.flush()?;
        Ok(Recorded {
            path: file.into_temp_path(),
            duration_seconds: self.samples.div_ceil(u64::from(RATE)) as u32,
            waveform,
        })
    }
}

fn pcm_bytes(samples: &[f32], output: &mut [u8]) {
    for (sample, bytes) in samples.iter().zip(output.chunks_exact_mut(2)) {
        bytes.copy_from_slice(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
}
