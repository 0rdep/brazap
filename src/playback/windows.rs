//! Media Foundation supplies decoding, the media clock and synchronized audio.
//! Rune's portable NativeSurface displays the latest BGRA frame. Unlike its
//! built-in macOS Player, this backend works on Windows without extra DLLs.

use super::Status;
use anyhow::{Context, Result};
use rune::{
    NativeSurface, Waker,
    native_surface::{ByteFrame, ColorInfo, Format},
};
use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{HANDLE, HMODULE, RECT, S_OK},
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP},
            Direct3D10::ID3D10Multithread,
            Direct3D11::*,
            Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
        },
        Media::MediaFoundation::*,
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Threading::SetEvent,
        },
    },
    core::{BSTR, Interface, implement},
};

enum Command {
    Toggle,
    Seek(f64),
    Mute,
    Volume(f64),
    Rate(f64),
    Event(u32, u32),
    Stop,
}

pub(crate) struct Player {
    surface: NativeSurface,
    status: Arc<Mutex<Status>>,
    commands: mpsc::Sender<Command>,
}

impl Player {
    pub fn new(file: tempfile::TempPath, waker: Waker) -> Self {
        Self::spawn(file, waker, false)
    }

    pub fn new_audio(file: tempfile::TempPath, waker: Waker) -> Self {
        Self::spawn(file, waker, true)
    }

    fn spawn(file: tempfile::TempPath, waker: Waker, audio: bool) -> Self {
        let surface = NativeSurface::new();
        surface.set_waker(waker.clone());
        let status = Arc::new(Mutex::new(Status {
            loading: true,
            ..Default::default()
        }));
        let (commands, receiver) = mpsc::channel();
        let player = Self {
            surface: surface.clone(),
            status: status.clone(),
            commands: commands.clone(),
        };
        let failed = status.clone();
        let failed_waker = waker.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("brazap-video".into())
            .spawn(move || {
                procwatch::alloc::tag_thread("video");
                if let Err(error) = run(&file, commands, receiver, &surface, &status, &waker, audio)
                {
                    *status.lock().unwrap_or_else(|e| e.into_inner()) = Status {
                        error: Some(format!("Could not play this video: {error:#}")),
                        ..Default::default()
                    };
                    waker();
                }
                // The decoder and COM apartment are gone before the temporary file.
                drop(file);
            })
        {
            failed.lock().unwrap_or_else(|e| e.into_inner()).error = Some(error.to_string());
            failed_waker();
        }
        player
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn view(&self) -> rune::El {
        rune::el()
            .w_full()
            .h_full()
            .native_surface(&self.surface, rune::SurfaceFit::Fit)
    }
    pub fn toggle(&self) {
        let _ = self.commands.send(Command::Toggle);
    }
    pub fn seek(&self, time: f64) {
        let _ = self.commands.send(Command::Seek(time));
    }
    pub fn set_volume(&self, volume: f64) {
        let _ = self.commands.send(Command::Volume(volume.clamp(0.0, 1.0)));
    }
    pub fn toggle_mute(&self) {
        let _ = self.commands.send(Command::Mute);
    }
    pub fn set_rate(&self, rate: f64) {
        let _ = self.commands.send(Command::Rate(rate));
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
    }
}

#[implement(IMFMediaEngineNotify)]
struct Notify(mpsc::Sender<Command>);

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, param1: usize, param2: u32) -> windows::core::Result<()> {
        if event == MF_MEDIA_ENGINE_EVENT_NOTIFYSTABLESTATE.0 as u32 {
            unsafe {
                SetEvent(HANDLE(param1 as *mut _))?;
            }
        } else {
            let _ = self.0.send(Command::Event(event, param2));
        }
        Ok(())
    }
}

struct Platform;
impl Platform {
    fn start() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                CoUninitialize();
                return Err(error).context("Windows Media Foundation is unavailable");
            }
        }
        Ok(Self)
    }
}
impl Drop for Platform {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

struct Engine(IMFMediaEngine);
impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Shutdown();
        }
    }
}

struct Frames {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    target: Option<ID3D11Texture2D>,
    staging: Option<ID3D11Texture2D>,
    size: (u32, u32),
    count: u64,
    textures: Option<procwatch::Lease>,
    max_side: u32,
}

impl Frames {
    fn new(max_side: u32) -> Result<Self> {
        let (mut device, mut context) = (None, None);
        unsafe {
            let create = |driver, device: &mut _, context: &mut _| {
                D3D11CreateDevice(
                    None,
                    driver,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(device),
                    None,
                    Some(context),
                )
            };
            create(D3D_DRIVER_TYPE_HARDWARE, &mut device, &mut context)
                .or_else(|_| create(D3D_DRIVER_TYPE_WARP, &mut device, &mut context))?;
            let device: ID3D11Device = device.context("No video device")?;
            let _ = device
                .cast::<ID3D10Multithread>()?
                .SetMultithreadProtected(true);
            Ok(Self {
                device,
                context: context.context("No video context")?,
                target: None,
                staging: None,
                size: (0, 0),
                count: 0,
                textures: None,
                max_side,
            })
        }
    }

    fn pull(&mut self, engine: &IMFMediaEngine) -> Result<Option<ByteFrame>> {
        unsafe {
            let mut pts = 0;
            // S_FALSE is success without a new frame; the generated Result wrapper
            // erases that distinction, so inspect the HRESULT directly.
            let hr =
                (Interface::vtable(engine).OnVideoStreamTick)(Interface::as_raw(engine), &mut pts);
            if hr != S_OK {
                return Ok(None);
            }
            let (mut width, mut height) = (0, 0);
            engine.GetNativeVideoSize(Some(&mut width), Some(&mut height))?;
            if width == 0 || height == 0 {
                return Ok(None);
            }
            let scale = (self.max_side as f64 / width.max(height) as f64).min(1.0);
            let size = (
                (width as f64 * scale).round().max(1.0) as u32,
                (height as f64 * scale).round().max(1.0) as u32,
            );
            if self.size != size {
                self.target = None;
                self.staging = None;
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: size.0,
                    Height: size.1,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32,
                    ..Default::default()
                };
                self.device
                    .CreateTexture2D(&desc, None, Some(&mut self.target))?;
                self.device.CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Usage: D3D11_USAGE_STAGING,
                        BindFlags: 0,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        ..desc
                    },
                    None,
                    Some(&mut self.staging),
                )?;
                self.size = size;
                self.textures = Some(
                    crate::diagnostics::VIDEO_TEXTURES
                        .track(size.0 as usize * size.1 as usize * 4 * 3),
                );
            }
            let target = self.target.as_ref().context("No video texture")?;
            let staging = self.staging.as_ref().context("No video readback texture")?;
            engine.TransferVideoFrame(
                target,
                None,
                &RECT {
                    left: 0,
                    top: 0,
                    right: size.0 as i32,
                    bottom: size.1 as i32,
                },
                Some(&MFARGB {
                    rgbAlpha: 255,
                    ..Default::default()
                }),
            )?;
            self.context.CopyResource(staging, target);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let row = size.0 as usize * 4;
            let mut bytes = vec![0; row * size.1 as usize];
            for (y, line) in bytes.chunks_exact_mut(row).enumerate() {
                let source = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
                line.copy_from_slice(std::slice::from_raw_parts(source, row));
            }
            self.context.Unmap(staging, 0);
            self.count += 1;
            Ok(Some(ByteFrame {
                width: size.0,
                height: size.1,
                format: Format::Bgra,
                color: ColorInfo::default(),
                planes: [bytes, Vec::new()],
            }))
        }
    }
}

impl Engine {
    fn audio(file: &std::path::Path, commands: mpsc::Sender<Command>) -> Result<Self> {
        unsafe {
            let notify: IMFMediaEngineNotify = Notify(commands).into();
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 1)?;
            let attributes = attributes.context("No audio attributes")?;
            attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)?;
            let engine =
                Self(factory.CreateInstance(MF_MEDIA_ENGINE_AUDIOONLY.0 as u32, &attributes)?);
            engine
                .0
                .SetSource(&BSTR::from(file.to_string_lossy().as_ref()))?;
            engine.0.Load()?;
            Ok(engine)
        }
    }
    fn open(
        file: &std::path::Path,
        frames: &Frames,
        commands: mpsc::Sender<Command>,
        muted: bool,
    ) -> Result<Self> {
        let engine = unsafe {
            let mut manager = None;
            let mut token = 0;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.context("No video device manager")?;
            manager.ResetDevice(&frames.device, token)?;
            let notify: IMFMediaEngineNotify = Notify(commands).into();
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 3)?;
            let attributes = attributes.context("No media attributes")?;
            attributes.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &manager)?;
            attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
            attributes.SetUINT32(
                &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
                DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
            )?;
            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)?;
            Engine(factory.CreateInstance(0, &attributes)?)
        };
        unsafe {
            engine.0.SetMuted(muted)?;
            engine
                .0
                .SetSource(&BSTR::from(file.to_string_lossy().as_ref()))?;
            engine.0.Load()?;
        }
        Ok(engine)
    }
}

fn run(
    file: &std::path::Path,
    commands: mpsc::Sender<Command>,
    receiver: mpsc::Receiver<Command>,
    surface: &NativeSurface,
    status: &Mutex<Status>,
    wake: &Waker,
    audio: bool,
) -> Result<()> {
    let _platform = Platform::start()?;
    let mut frames = if audio {
        None
    } else {
        Some(Frames::new(1280)?)
    };
    let engine = if let Some(frames) = &frames {
        Engine::open(file, frames, commands, false)?
    } else {
        Engine::audio(file, commands)?
    };
    let mut wanted_playing = true;
    let mut ready = false;
    let mut last_tick = Instant::now();
    loop {
        let command = if ready && !wanted_playing && !unsafe { engine.0.IsSeeking().as_bool() } {
            receiver.recv().ok()
        } else {
            match receiver.recv_timeout(Duration::from_millis(if audio { 100 } else { 16 })) {
                Ok(command) => Some(command),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        };
        unsafe {
            match command {
                Some(Command::Stop) => break,
                Some(Command::Toggle) => {
                    wanted_playing = !wanted_playing;
                    if wanted_playing {
                        if engine.0.IsEnded().as_bool() {
                            engine.0.SetCurrentTime(0.0)?;
                        }
                        if ready {
                            engine.0.Play()?;
                        }
                    } else {
                        engine.0.Pause()?;
                    }
                }
                Some(Command::Seek(time)) => {
                    let duration = engine.0.GetDuration();
                    if time.is_finite() && duration.is_finite() {
                        engine.0.SetCurrentTime(time.clamp(0.0, duration))?;
                    }
                }
                Some(Command::Volume(volume)) => {
                    engine.0.SetVolume(volume)?;
                    engine.0.SetMuted(volume == 0.0)?;
                }
                Some(Command::Mute) => {
                    engine.0.SetMuted(!engine.0.GetMuted().as_bool())?;
                }
                Some(Command::Rate(rate)) => {
                    if [1.0, 1.5, 2.0].contains(&rate) {
                        engine.0.SetPlaybackRate(rate)?;
                    }
                }
                Some(Command::Event(event, detail)) => {
                    if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
                        anyhow::bail!("Windows decoder error 0x{detail:08X}");
                    }
                    if event == MF_MEDIA_ENGINE_EVENT_CANPLAY.0 as u32 {
                        ready = true;
                        if wanted_playing {
                            engine.0.Play()?;
                        }
                    }
                    if event == MF_MEDIA_ENGINE_EVENT_ENDED.0 as u32 {
                        wanted_playing = false;
                    }
                }
                None => {}
            }
            // Event bursts must not drive unbounded texture readbacks.
            if last_tick.elapsed() >= Duration::from_millis(15) {
                if let Some(frames) = &mut frames
                    && let Some(frame) = frames.pull(&engine.0)?
                {
                    surface
                        .push_bytes(frame, frames.count)
                        .map_err(anyhow::Error::msg)?;
                }
                last_tick = Instant::now();
            }
            let duration = engine.0.GetDuration();
            let position = engine.0.GetCurrentTime();
            let next = Status {
                loading: !ready,
                playing: ready && wanted_playing && !engine.0.IsEnded().as_bool(),
                ended: engine.0.IsEnded().as_bool(),
                muted: engine.0.GetMuted().as_bool(),
                volume: engine.0.GetVolume(),
                position: if position.is_finite() {
                    (position * 10.0).floor() / 10.0
                } else {
                    0.0
                },
                duration: duration.is_finite().then_some(duration.max(0.0)),
                error: None,
                rate: engine.0.GetPlaybackRate(),
                dimensions: frames
                    .as_ref()
                    .filter(|f| f.size.0 > 0 && f.size.1 > 0)
                    .map(|f| [f.size.0, f.size.1]),
            };
            let mut previous = status.lock().unwrap_or_else(|e| e.into_inner());
            if *previous != next {
                *previous = next;
                drop(previous);
                wake();
            }
        }
    }
    Ok(())
}

/// Decode one quiet poster when WhatsApp omitted its embedded thumbnail.
pub(super) fn poster(bytes: Vec<u8>) -> Result<Vec<u8>> {
    use std::io::{Cursor, Write};
    procwatch::alloc::tag_thread("video posters");
    let mut file = tempfile::Builder::new()
        .prefix("brazap-poster-")
        .suffix(".mp4")
        .tempfile()?;
    file.write_all(&bytes)?;
    drop(bytes);
    let file = file.into_temp_path();
    let _platform = Platform::start()?;
    let mut frames = Frames::new(360)?;
    let (commands, receiver) = mpsc::channel();
    let engine = Engine::open(&file, &frames, commands, true)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(16)) {
            Ok(Command::Event(event, detail)) if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 => {
                anyhow::bail!("Video poster decoder error 0x{detail:08X}");
            }
            Ok(Command::Event(event, _)) if event == MF_MEDIA_ENGINE_EVENT_CANPLAY.0 as u32 => unsafe {
                engine.0.Play()?;
            },
            _ => {}
        }
        if let Some(frame) = frames.pull(&engine.0)? {
            let [mut rgba, _] = frame.planes;
            for pixel in rgba.chunks_exact_mut(4) {
                pixel.swap(0, 2);
                pixel[3] = 255;
            }
            let image = image::RgbaImage::from_raw(frame.width, frame.height, rgba)
                .context("Invalid video poster frame")?;
            let mut output = Cursor::new(Vec::new());
            image.write_to(&mut output, image::ImageFormat::Png)?;
            return Ok(output.into_inner());
        }
    }
    anyhow::bail!("Video poster timed out")
}
