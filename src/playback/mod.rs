//! One player per open viewer. Originals stay in SQLite; the temporary playback
//! file belongs to the player and is removed when its decoder closes.

mod voice;
#[cfg(target_os = "windows")]
mod windows;

pub(crate) use voice::prepare as prepare_audio;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub(crate) struct Status {
    pub loading: bool,
    pub playing: bool,
    pub ended: bool,
    pub muted: bool,
    pub volume: f64,
    pub position: f64,
    pub duration: Option<f64>,
    pub error: Option<String>,
    pub rate: f64,
    pub dimensions: Option<[u32; 2]>,
}

#[cfg(target_os = "windows")]
pub(crate) use windows::Player;

/// A poster frame for a video WhatsApp sent without a thumbnail, as PNG bytes.
pub(crate) async fn poster(bytes: Vec<u8>) -> anyhow::Result<Option<Vec<u8>>> {
    static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let _permit = SLOTS.acquire().await?;
    tokio::task::spawn_blocking(move || {
        // Windows decodes every format it supports; the bundled H.264 decoder covers the
        // other platforms, and any video Windows cannot open.
        #[cfg(target_os = "windows")]
        match windows::poster(bytes.clone()) {
            Ok(png) => return Ok(Some(png)),
            Err(error) => log::debug!("Windows could not decode a video poster: {error:#}"),
        }
        match crate::media_convert::first_frame_png(&bytes, 360) {
            Ok(png) => Ok(Some(png)),
            Err(error) => {
                log::debug!("Could not decode a video poster: {error:#}");
                Ok(None)
            }
        }
    })
    .await?
}

#[cfg(not(target_os = "windows"))]
pub(crate) struct Player {
    clip: rune::Player,
    _file: tempfile::TempPath,
}

#[cfg(not(target_os = "windows"))]
impl Player {
    pub fn new(file: tempfile::TempPath, waker: rune::Waker) -> Self {
        let clip = rune::Player::file(&file);
        clip.set_waker(waker);
        clip.set_muted(false);
        clip.play();
        Self { clip, _file: file }
    }

    pub fn new_audio(file: tempfile::TempPath, waker: rune::Waker) -> Self {
        let clip = rune::Player::audio(file.to_path_buf());
        clip.set_waker(waker);
        clip.play();
        Self { clip, _file: file }
    }

    pub fn status(&self) -> Status {
        let state = self.clip.state();
        Status {
            loading: matches!(state, rune::PlayerState::Loading),
            playing: self.clip.is_playing(),
            ended: matches!(state, rune::PlayerState::Ended),
            muted: self.clip.muted(),
            volume: if self.clip.muted() { 0.0 } else { 1.0 },
            position: (self.clip.time() * 10.0).floor() / 10.0,
            duration: self.clip.duration(),
            error: match state {
                rune::PlayerState::Failed(error) => Some(error),
                _ => None,
            },
            rate: self.clip.rate() as f64,
            dimensions: self.clip.natural_size().map(|(w, h)| [w, h]),
        }
    }

    pub fn view(&self) -> rune::El {
        rune::el()
            .w_full()
            .h_full()
            .video(&self.clip, rune::SurfaceFit::Fit)
    }

    pub fn toggle(&self) {
        if self.clip.is_playing() {
            self.clip.pause();
        } else {
            self.clip.play();
        }
    }

    pub fn seek(&self, time: f64) {
        self.clip.seek(time);
    }
    pub fn set_rate(&self, rate: f64) {
        self.clip.set_rate(rate as f32);
    }

    pub fn toggle_mute(&self) {
        self.clip.set_muted(!self.clip.muted());
    }
}
