//! Real-time device callbacks only move samples through bounded, lock-free rings.
//! Resampling and channel delivery stay on a dedicated call-audio worker.
use anyhow::{Context, Result};
use cpal::{
    ErrorKind, SampleFormat, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use whatsapp_rust::async_channel::{self, Receiver, Sender};

const RATE: u32 = 16_000;
const FRAME: usize = 960;

#[derive(Default)]
struct Control {
    stop: AtomicBool,
    muted: AtomicBool,
    failed: AtomicBool,
    error: Mutex<Option<String>>,
}

pub(super) struct Audio {
    control: Arc<Control>,
    input: Receiver<Vec<i16>>,
    output: Sender<Vec<i16>>,
}
impl Drop for Audio {
    fn drop(&mut self) {
        self.control.stop.store(true, Ordering::Release);
    }
}
impl Audio {
    pub async fn start() -> Result<Self> {
        let control = Arc::new(Control::default());
        let (input_tx, input) = async_channel::bounded(3);
        let (output, output_rx) = async_channel::bounded(4);
        let (ready, wait) = tokio::sync::oneshot::channel();
        let worker = control.clone();
        let audio = Self {
            control,
            input,
            output,
        };
        std::thread::Builder::new()
            .name("brazap-call-audio".into())
            .spawn(move || {
                procwatch::alloc::tag_thread("call audio");
                if let Err(error) = run(&worker, input_tx, output_rx, ready) {
                    log::warn!("Call audio failed: {error:#}");
                    *worker.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                        "Microphone or speaker unavailable. Check audio devices and permissions."
                            .into(),
                    );
                }
            })?;
        tokio::time::timeout(Duration::from_secs(10), wait)
            .await
            .context("Audio devices took too long to open")?
            .context("Could not open audio devices")??;
        Ok(audio)
    }
    pub fn source(&self) -> Receiver<Vec<i16>> {
        self.input.clone()
    }
    pub fn sink(&self) -> Sender<Vec<i16>> {
        self.output.clone()
    }
    pub fn mute(&self, muted: bool) {
        self.control.muted.store(muted, Ordering::Relaxed);
    }
    pub fn error(&self) -> Option<String> {
        self.control
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

fn stream_error(control: &Control, error: cpal::Error) {
    if !matches!(
        error.kind(),
        ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied
    ) {
        control.failed.store(true, Ordering::Relaxed);
    }
}

fn input<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut ring: HeapProd<f32>,
    control: Arc<Control>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels);
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            for frame in data.chunks_exact(channels) {
                let mono =
                    frame.iter().map(|v| v.to_sample::<f32>()).sum::<f32>() / channels as f32;
                let _ = ring.try_push(mono);
            }
        },
        move |e| stream_error(&control, e),
        None,
    )?)
}

fn output<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut ring: HeapCons<f32>,
    control: Arc<Control>,
) -> Result<cpal::Stream>
where
    T: SizedSample + cpal::FromSample<f32>,
{
    let channels = usize::from(config.channels);
    Ok(device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            for frame in data.chunks_exact_mut(channels) {
                let sample = T::from_sample(ring.try_pop().unwrap_or(0.0));
                frame.fill(sample);
            }
        },
        move |e| stream_error(&control, e),
        None,
    )?)
}

// Linear interpolation with a carried fractional position across input blocks.
// State is bounded to the preceding sample and the next output position.
struct Resampler {
    previous: Option<f32>,
    next: f64,
    step: f64,
}
impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Self {
            previous: None,
            next: 0.0,
            step: from as f64 / to as f64,
        }
    }
    fn push(&mut self, current: f32, mut emit: impl FnMut(f32)) {
        let Some(previous) = self.previous.replace(current) else {
            return;
        };
        while self.next < 1.0 {
            emit(previous + (current - previous) * self.next as f32);
            self.next += self.step;
        }
        self.next -= 1.0;
    }
}

fn run(
    control: &Arc<Control>,
    input_tx: Sender<Vec<i16>>,
    output_rx: Receiver<Vec<i16>>,
    ready: tokio::sync::oneshot::Sender<Result<()>>,
) -> Result<()> {
    let host = cpal::default_host();
    let microphone = host
        .default_input_device()
        .context("No microphone is available")?;
    let speaker = host
        .default_output_device()
        .context("No speaker is available")?;
    let input_config = microphone.default_input_config()?;
    let output_config = speaker.default_output_config()?;
    let input_rate = input_config.sample_rate();
    let output_rate = output_config.sample_rate();
    let (input_ring, mut input_samples) = HeapRb::<f32>::new(input_rate as usize / 4).split();
    let (mut output_samples, output_ring) = HeapRb::<f32>::new(output_rate as usize / 4).split();
    #[cfg(target_os = "windows")]
    for _ in 0..(output_rate / 10) {
        let _ = output_samples.try_push(0.0);
    }
    macro_rules! capture {
        ($t:ty) => {
            input::<$t>(
                &microphone,
                input_config.config(),
                input_ring,
                control.clone(),
            )?
        };
    }
    macro_rules! playback {
        ($t:ty) => {
            output::<$t>(
                &speaker,
                output_config.config(),
                output_ring,
                control.clone(),
            )?
        };
    }
    let input_stream = match input_config.sample_format() {
        SampleFormat::I8 => capture!(i8),
        SampleFormat::I16 => capture!(i16),
        SampleFormat::I32 => capture!(i32),
        SampleFormat::U8 => capture!(u8),
        SampleFormat::U16 => capture!(u16),
        SampleFormat::U32 => capture!(u32),
        SampleFormat::F32 => capture!(f32),
        SampleFormat::F64 => capture!(f64),
        _ => anyhow::bail!("Unsupported microphone sample format"),
    };
    let output_stream = match output_config.sample_format() {
        SampleFormat::I8 => playback!(i8),
        SampleFormat::I16 => playback!(i16),
        SampleFormat::I32 => playback!(i32),
        SampleFormat::U8 => playback!(u8),
        SampleFormat::U16 => playback!(u16),
        SampleFormat::U32 => playback!(u32),
        SampleFormat::F32 => playback!(f32),
        SampleFormat::F64 => playback!(f64),
        _ => anyhow::bail!("Unsupported speaker sample format"),
    };
    input_stream.play()?;
    output_stream.play()?;
    let _ = ready.send(Ok(()));
    let mut capture = Resampler::new(input_rate, RATE);
    let mut playback = Resampler::new(RATE, output_rate);
    let mut frame = Vec::with_capacity(FRAME);
    while !control.stop.load(Ordering::Acquire) {
        anyhow::ensure!(
            !control.failed.load(Ordering::Relaxed),
            "Audio device disconnected"
        );
        while let Some(sample) = input_samples.try_pop() {
            capture.push(sample, |sample| {
                frame.push(if control.muted.load(Ordering::Relaxed) {
                    0
                } else {
                    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
                });
                if frame.len() == FRAME {
                    let _ =
                        input_tx.try_send(std::mem::replace(&mut frame, Vec::with_capacity(FRAME)));
                }
            });
        }
        while let Ok(frame) = output_rx.try_recv() {
            for sample in frame {
                playback.push(sample as f32 / i16::MAX as f32, |sample| {
                    let _ = output_samples.try_push(sample);
                });
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
