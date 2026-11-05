//! Native camera capture and H.264. Only the latest small preview is retained;
//! encoded queues are bounded and request an IDR after backpressure.
use super::PreviewFrame;
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{RgbImage, imageops};
use nokhwa::{
    Camera,
    pixel_format::RgbFormat,
    utils::{
        CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
    },
};
use openh264::{
    decoder::Decoder,
    encoder::{
        BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, Profile, RateControlMode,
    },
    formats::{RgbSliceU8, YUVBuffer, YUVSource},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use whatsapp_rust::{
    async_channel::{self, Receiver, Sender},
    voip::{TimedVideoFrame, VideoFrame, VideoSource},
};

const FPS: u32 = 15;
#[derive(Default)]
struct Control {
    stop: AtomicBool,
    black: AtomicBool,
    capture_stop: AtomicBool,
    capture_done: AtomicBool,
    keyframe: AtomicBool,
    lost: AtomicBool,
    error: Mutex<Option<String>>,
    local: Mutex<PreviewFrame>,
    remote: Mutex<PreviewFrame>,
}
#[derive(Clone)]
pub(super) struct Source {
    timed: Receiver<TimedVideoFrame>,
    legacy: Receiver<Vec<u8>>,
}
impl VideoSource for Source {
    fn frames(&self) -> Receiver<Vec<u8>> {
        self.legacy.clone()
    }
    fn timed_frames(&self) -> Option<Receiver<TimedVideoFrame>> {
        Some(self.timed.clone())
    }
    fn rtp_timestamp_stride(&self) -> u32 {
        90_000 / FPS
    }
}
pub(super) struct Video {
    control: Arc<Control>,
    source: Source,
    sink: Sender<VideoFrame>,
}
impl Drop for Video {
    fn drop(&mut self) {
        self.control.stop.store(true, Ordering::Release);
    }
}
impl Video {
    pub async fn start(demo: bool) -> Result<Self> {
        let camera_allowed = if !demo {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let tx = Arc::new(Mutex::new(Some(tx)));
            nokhwa::nokhwa_initialize(move |allowed| {
                if let Some(tx) = tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    let _ = tx.send(allowed);
                }
            });
            match tokio::time::timeout(Duration::from_secs(8), rx).await {
                Ok(Ok(true)) => true,
                _ => {
                    log::warn!("Camera permission unavailable; using black video");
                    false
                }
            }
        } else {
            false
        };
        match Self::start_capture(camera_allowed, demo).await {
            Ok(video) => Ok(video),
            Err(error) if camera_allowed => {
                log::warn!("Camera startup failed; using black video: {error:#}");
                Self::start_capture(false, demo).await
            }
            Err(error) => Err(error),
        }
    }
    async fn start_capture(camera_allowed: bool, demo: bool) -> Result<Self> {
        let control = Arc::new(Control::default());
        let (source_tx, timed) = async_channel::bounded(2);
        let (_unused, legacy) = async_channel::bounded(1);
        let (sink, received) = async_channel::bounded(2);
        let (ready, wait) = tokio::sync::oneshot::channel();
        let device = Self {
            control: control.clone(),
            source: Source { timed, legacy },
            sink,
        };
        let capture = control.clone();
        let loopback = demo.then(|| device.sink.clone());
        std::thread::Builder::new()
            .name("brazap-call-camera".into())
            .spawn(move || {
                procwatch::alloc::tag_thread("call camera");
                let result = capture_frames(&capture, source_tx, ready, camera_allowed, loopback);
                capture.capture_done.store(true, Ordering::Release);
                if let Err(e) = result {
                    log::warn!("Call camera failed: {e:#}");
                    *capture.error.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some("Could not encode the call video.".into());
                }
            })?;
        {
            let decode = control.clone();
            std::thread::Builder::new()
                .name("brazap-call-video".into())
                .spawn(move || {
                    procwatch::alloc::tag_thread("call video decoder");
                    if let Err(e) = decode_frames(&decode, received) {
                        log::warn!("Call video decoder failed: {e:#}");
                        *decode.error.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some("Could not decode the call video.".into());
                    }
                })?;
        }
        tokio::time::timeout(Duration::from_secs(12), wait)
            .await
            .context("Camera took too long to open")?
            .context("Could not open camera")??;
        Ok(device)
    }
    pub fn is_black(&self) -> bool {
        self.control.black.load(Ordering::Acquire)
    }
    pub fn source(&self) -> Source {
        self.source.clone()
    }
    pub fn sink(&self) -> Sender<VideoFrame> {
        self.sink.clone()
    }
    pub fn camera_stopped(&self) -> bool {
        self.control.capture_done.load(Ordering::Acquire)
    }
    pub fn stop_camera(&self) {
        self.control.capture_stop.store(true, Ordering::Release);
        *self.control.local.lock().unwrap_or_else(|e| e.into_inner()) = PreviewFrame::default();
    }
    pub fn keyframe(&self) {
        self.control.keyframe.store(true, Ordering::Relaxed);
    }
    pub fn needs_keyframe(&self) -> bool {
        self.control.lost.swap(false, Ordering::Relaxed)
    }
    pub fn error(&self) -> Option<String> {
        self.control
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn preview(&self, local: bool, after: u64) -> Option<PreviewFrame> {
        let frame = if local {
            &self.control.local
        } else {
            &self.control.remote
        };
        let frame = frame.lock().unwrap_or_else(|e| e.into_inner());
        (frame.sequence > after).then(|| frame.clone())
    }
}

fn preview(frame: &RgbImage, target: &Mutex<PreviewFrame>) -> Result<()> {
    let small = imageops::thumbnail(frame, 640, 360);
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 75).encode(
        small.as_raw(),
        small.width(),
        small.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    let mut target = target.lock().unwrap_or_else(|e| e.into_inner());
    static NEXT_FRAME: AtomicU64 = AtomicU64::new(1);
    target.sequence = NEXT_FRAME.fetch_add(1, Ordering::Relaxed);
    target.jpeg = STANDARD.encode(bytes);
    Ok(())
}

fn open_camera() -> Result<Camera> {
    let format = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(
        CameraFormat::new(Resolution::new(640, 480), FrameFormat::MJPEG, 30),
    ));
    let mut camera = Camera::new(CameraIndex::Index(0), format).or_else(|_| {
        Camera::new(
            CameraIndex::Index(0),
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate),
        )
    })?;
    let mut formats = camera.compatible_camera_formats()?;
    formats.retain(|f| {
        f.width() <= 1280 && f.height() <= 720 && f.frame_rate() >= FPS && f.frame_rate() <= 30
    });
    formats.sort_by_key(|f| {
        (
            f.width().abs_diff(640) + f.height().abs_diff(480),
            f.frame_rate().abs_diff(30),
        )
    });
    ensure!(
        !formats.is_empty(),
        "No supported camera format at 720p or below"
    );
    if let Some(format) = formats.first() {
        camera.set_camera_requset(RequestedFormat::new::<RgbFormat>(
            RequestedFormatType::Exact(*format),
        ))?;
    }
    camera.open_stream()?;
    Ok(camera)
}

fn camera_frame(camera: &mut Camera) -> Result<RgbImage> {
    let frame = camera.frame()?;
    let resolution = frame.resolution();
    ensure!(
        resolution.width() <= 4096 && resolution.height() <= 2160,
        "Camera resolution exceeds call limit"
    );
    let mut rgb = frame.decode_image::<RgbFormat>()?;
    if rgb.width() > 1280 || rgb.height() > 720 {
        rgb = imageops::thumbnail(&rgb, 1280, 720);
    }
    let (w, h) = (rgb.width() & !1, rgb.height() & !1);
    ensure!(w >= 2 && h >= 2, "Camera returned an empty frame");
    if rgb.dimensions() != (w, h) {
        rgb = imageops::crop_imm(&rgb, 0, 0, w, h).to_image();
    }
    Ok(rgb)
}

fn capture_frames(
    control: &Control,
    send: Sender<TimedVideoFrame>,
    ready: tokio::sync::oneshot::Sender<Result<()>>,
    camera_allowed: bool,
    loopback: Option<Sender<VideoFrame>>,
) -> Result<()> {
    let mut camera = if camera_allowed {
        match open_camera() {
            Ok(camera) => Some(camera),
            Err(error) => {
                log::warn!("Camera unavailable; using black video: {error:#}");
                None
            }
        }
    } else {
        None
    };
    control.black.store(camera.is_none(), Ordering::Release);
    let black = RgbImage::new(640, 360);
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(700_000))
        .max_frame_rate(FrameRate::from_hz(FPS as f32))
        .profile(Profile::Baseline)
        .rate_control_mode(RateControlMode::Bitrate)
        .intra_frame_period(IntraFramePeriod::from_num_frames(FPS * 3))
        .num_threads(1);
    let mut encoder = Encoder::with_api_config(openh264::OpenH264API::from_source(), config)?;
    let mut ready = Some(ready);
    let started = Instant::now();
    let mut last_preview = Instant::now() - Duration::from_secs(1);
    let mut yuv = None;
    while !control.stop.load(Ordering::Acquire) && !control.capture_stop.load(Ordering::Acquire) {
        let frame_start = Instant::now();
        let actual = if let Some(device) = camera.as_mut() {
            match camera_frame(device) {
                Ok(frame) => Some(frame),
                Err(error) => {
                    log::warn!("Camera capture stopped; using black video: {error:#}");
                    let _ = device.stop_stream();
                    camera = None;
                    *control.local.lock().unwrap_or_else(|e| e.into_inner()) =
                        PreviewFrame::default();
                    control.black.store(true, Ordering::Release);
                    encoder.force_intra_frame();
                    None
                }
            }
        } else {
            None
        };
        let rgb = actual.as_ref().unwrap_or(&black);
        let (w, h) = rgb.dimensions();
        if last_preview.elapsed() >= Duration::from_millis(100)
            && (actual.is_some()
                || control
                    .local
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .sequence
                    == 0)
        {
            preview(rgb, &control.local)?;
            last_preview = Instant::now();
        }
        {
            let yuv = yuv.get_or_insert_with(|| YUVBuffer::new(w as usize, h as usize));
            if yuv.dimensions() != (w as usize, h as usize) {
                *yuv = YUVBuffer::new(w as usize, h as usize);
            }
            yuv.read_rgb8(RgbSliceU8::new(rgb.as_raw(), (w as usize, h as usize)));
            if control.keyframe.swap(false, Ordering::Relaxed) {
                encoder.force_intra_frame();
            }
            let bytes = encoder.encode(yuv)?.to_vec();
            if !bytes.is_empty() {
                let timestamp =
                    (started.elapsed().as_micros().saturating_mul(90_000) / 1_000_000) as u32;
                let dropped = if let Some(loopback) = &loopback {
                    loopback
                        .try_send(
                            VideoFrame::builder()
                                .data(bytes)
                                .keyframe(true)
                                .orientation(0)
                                .timestamp(timestamp)
                                .generation(0)
                                .build(),
                        )
                        .is_err()
                } else {
                    let frame = TimedVideoFrame::builder()
                        .data(bytes)
                        .timestamp(timestamp)
                        .captured_at(whatsapp_rust::wacore::time::Instant::now())
                        .build();
                    send.try_send(frame).is_err()
                };
                if dropped {
                    control.keyframe.store(true, Ordering::Relaxed);
                }
            }
        }
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(()));
        }
        if let Some(delay) =
            Duration::from_secs_f64(1.0 / FPS as f64).checked_sub(frame_start.elapsed())
        {
            std::thread::sleep(delay);
        }
    }
    if let Some(camera) = camera.as_mut() {
        let _ = camera.stop_stream();
    }
    Ok(())
}

fn decode_frames(control: &Control, receive: Receiver<VideoFrame>) -> Result<()> {
    let mut decoder = Decoder::new()?;
    let mut last_preview = Instant::now() - Duration::from_secs(1);
    while !control.stop.load(Ordering::Acquire) {
        let frame = match receive.try_recv() {
            Ok(f) => f,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
        };
        if frame.data.len() > 4 * 1024 * 1024 {
            control.lost.store(true, Ordering::Relaxed);
            continue;
        }
        let decoded = match decoder.decode(&frame.data) {
            Ok(Some(frame)) => frame,
            Ok(None) => continue,
            Err(_) => {
                control.lost.store(true, Ordering::Relaxed);
                continue;
            }
        };
        if last_preview.elapsed() < Duration::from_millis(100) {
            continue;
        }
        let (w, h) = decoded.dimensions();
        ensure!(
            w <= 4096 && h <= 2160 && w > 0 && h > 0,
            "Peer video exceeds call limit"
        );
        let mut rgb = vec![0; decoded.rgb8_len()];
        decoded.write_rgb8(&mut rgb);
        let rgb =
            RgbImage::from_raw(w as u32, h as u32, rgb).context("Invalid call video dimensions")?;
        let rgb = match frame.orientation % 4 {
            1 => imageops::rotate90(&rgb),
            2 => imageops::rotate180(&rgb),
            3 => imageops::rotate270(&rgb),
            _ => rgb,
        };
        preview(&rgb, &control.remote)?;
        last_preview = Instant::now();
    }
    Ok(())
}
