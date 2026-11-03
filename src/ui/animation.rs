//! Animated WebP stickers share one sleeping decoder worker and reuse pixel slots.
use image::{AnimationDecoder, ImageDecoder, codecs::webp::WebPDecoder};
use rune::Pixels;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::Cursor,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const MAX_ITEMS: usize = 32;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_ACTIVE: usize = 8;
#[derive(Default)]
struct Stats {
    frames: AtomicU64,
    active: AtomicUsize,
    bytes: AtomicUsize,
}
static STATS: OnceLock<Arc<Stats>> = OnceLock::new();
pub(super) fn report() -> Value {
    let Some(stats) = STATS.get() else {
        return json!({});
    };
    json!({"frames":stats.frames.load(Ordering::Relaxed),"active":stats.active.load(Ordering::Relaxed),"estimated_bytes":stats.bytes.load(Ordering::Relaxed),"active_limit":MAX_ACTIVE})
}

// VP8X bit 1 is the animation flag (image-webp's extended-header parser).
pub(super) fn is_animated(bytes: &[u8]) -> bool {
    crate::assets::animated_webp(bytes)
}
enum Command {
    Load(String, Arc<[u8]>, Pixels, u32),
    Active(Vec<String>),
    Remove(String),
    Stop,
}
struct Handle {
    pixels: Pixels,
    len: usize,
}
pub(super) struct Player {
    tx: mpsc::Sender<Command>,
    handles: HashMap<String, Handle>,
    order: VecDeque<String>,
    active: Vec<String>,
}
impl Default for Player {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        let stats = STATS.get_or_init(|| Arc::new(Stats::default())).clone();
        std::thread::Builder::new()
            .name("brazap-stickers".into())
            .stack_size(512 * 1024)
            .spawn(move || worker(rx, stats))
            .expect("Sticker decoder worker");
        Self {
            tx,
            handles: HashMap::new(),
            order: VecDeque::new(),
            active: Vec::new(),
        }
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Stop);
    }
}
impl Player {
    pub(super) fn contains(&self, key: &str) -> bool {
        self.handles.contains_key(key)
    }
    pub(super) fn pixels(&self, key: &str) -> Option<&Pixels> {
        self.handles.get(key).map(|h| &h.pixels)
    }
    pub(super) fn load(&mut self, key: String, bytes: Arc<[u8]>) -> bool {
        if self.contains(&key) {
            return true;
        }
        if bytes.len() > 8 * 1024 * 1024 || !is_animated(&bytes) {
            return false;
        }
        let Ok(decoder) = WebPDecoder::new(Cursor::new(bytes.as_ref())) else {
            return false;
        };
        let (w, h) = decoder.dimensions();
        if w == 0 || h == 0 || w > 1024 || h > 1024 {
            return false;
        }
        let side = if key.starts_with("sticker:") {
            128
        } else if key.starts_with("image:") {
            512
        } else {
            256
        };
        let pixels = Pixels::new();
        self.handles.insert(
            key.clone(),
            Handle {
                pixels: pixels.clone(),
                len: bytes.len(),
            },
        );
        self.order.push_back(key.clone());
        let _ = self.tx.send(Command::Load(key, bytes.into(), pixels, side));
        while self.handles.len() > MAX_ITEMS
            || self.handles.values().map(|h| h.len).sum::<usize>() > MAX_BYTES
        {
            let index = self
                .order
                .iter()
                .position(|k| !self.active.contains(k))
                .unwrap_or(0);
            let Some(key) = self.order.remove(index) else {
                break;
            };
            self.handles.remove(&key);
            let _ = self.tx.send(Command::Remove(key));
        }
        true
    }
    pub(super) fn activate(&mut self, keys: Vec<String>) {
        let mut seen = HashSet::new();
        let keys: Vec<_> = keys
            .into_iter()
            .filter(|k| self.handles.contains_key(k) && seen.insert(k.clone()))
            .take(MAX_ACTIVE)
            .collect();
        if keys != self.active {
            self.active = keys.clone();
            let _ = self.tx.send(Command::Active(keys));
        }
    }
    pub(super) fn clear(&mut self) {
        for (key, _) in self.handles.drain() {
            let _ = self.tx.send(Command::Remove(key));
        }
        self.order.clear();
        self.active.clear();
        let _ = self.tx.send(Command::Active(Vec::new()));
    }
}
struct Entry {
    bytes: Arc<[u8]>,
    pixels: Pixels,
    frames: Option<image::Frames<'static>>,
    next: Instant,
    width: u32,
    height: u32,
    side: u32,
    active: bool,
    failed: bool,
}
impl Entry {
    fn decoder(&self) -> Option<image::Frames<'static>> {
        WebPDecoder::new(Cursor::new(self.bytes.clone()))
            .ok()
            .map(AnimationDecoder::into_frames)
    }
    fn frame(&mut self, stats: &Stats) {
        if self.failed {
            return;
        }
        if self.frames.is_none() {
            self.frames = self.decoder();
        }
        let mut frame = self.frames.as_mut().and_then(Iterator::next);
        if frame.is_none() {
            self.frames = self.decoder();
            frame = self.frames.as_mut().and_then(Iterator::next);
        }
        match frame {
            Some(Ok(frame)) => {
                let (numerator, denominator) = frame.delay().numer_denom_ms();
                let delay = (numerator as f64 / denominator.max(1) as f64).clamp(33.0, 10_000.0);
                self.next = Instant::now() + Duration::from_secs_f64(delay / 1000.0);
                let rgba = frame.into_buffer();
                let rgba = if rgba.width() > self.side || rgba.height() > self.side {
                    image::imageops::thumbnail(&rgba, self.side, self.side)
                } else {
                    rgba
                };
                let (w, h) = rgba.dimensions();
                self.pixels.set(w, h, rgba.into_raw());
                stats.frames.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                self.failed = true;
                self.frames = None;
            }
        }
        if !self.active {
            self.frames = None;
        }
    }
}
fn worker(rx: mpsc::Receiver<Command>, stats: Arc<Stats>) {
    procwatch::alloc::tag_thread("sticker animations");
    let mut entries = HashMap::<String, Entry>::new();
    let mut active = HashSet::<String>::new();
    loop {
        let next = entries
            .values()
            .filter(|e| e.active && !e.failed)
            .map(|e| e.next)
            .min();
        let result = match next {
            Some(next) => rx
                .recv_timeout(next.saturating_duration_since(Instant::now()))
                .map(Some),
            None => rx
                .recv()
                .map(Some)
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        let command = match result {
            Ok(c) => c,
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(_) => break,
        };
        match command {
            Some(Command::Stop) => break,
            Some(Command::Remove(key)) => {
                entries.remove(&key);
            }
            Some(Command::Active(keys)) => {
                active = keys.into_iter().collect();
                for (key, entry) in &mut entries {
                    entry.active = active.contains(key);
                    if !entry.active {
                        entry.frames = None;
                    } else {
                        entry.next = Instant::now();
                    }
                }
            }
            Some(Command::Load(key, bytes, pixels, side)) => {
                let Ok(decoder) = WebPDecoder::new(Cursor::new(bytes.clone())) else {
                    continue;
                };
                let (width, height) = decoder.dimensions();
                let mut entry = Entry {
                    bytes,
                    pixels,
                    frames: Some(decoder.into_frames()),
                    next: Instant::now(),
                    width,
                    height,
                    side,
                    active: active.contains(&key),
                    failed: false,
                };
                entry.frame(&stats);
                entries.insert(key, entry);
            }
            None => {}
        }
        for entry in entries
            .values_mut()
            .filter(|e| e.active && !e.failed && e.next <= Instant::now())
        {
            entry.frame(&stats);
        }
        stats.active.store(
            entries.values().filter(|e| e.active && !e.failed).count(),
            Ordering::Relaxed,
        );
        stats.bytes.store(
            entries
                .values()
                .map(|e| {
                    let (w, h) = e.pixels.size();
                    e.bytes.len()
                        + w as usize * h as usize * 4
                        + if e.frames.is_some() {
                            e.width as usize * e.height as usize * 8
                        } else {
                            0
                        }
                })
                .sum(),
            Ordering::Relaxed,
        );
    }
    stats.active.store(0, Ordering::Relaxed);
    stats.bytes.store(0, Ordering::Relaxed);
}

impl super::State {
    pub(super) fn update_animations(&mut self) {
        let keys = if self.preferences.open
            || self.section != super::Section::Messages
            || !self.authenticated
        {
            Vec::new()
        } else if let Some(message) = &self.viewer {
            vec![
                crate::assets::AssetRequest::Image {
                    chat: message.chat.clone(),
                    sender: message.sender.clone(),
                    id: message.id.clone(),
                }
                .key(),
            ]
        } else if self.picker.open {
            self.picker.animation_keys.clone()
        } else if self.details {
            self.gallery
                .iter()
                .take(9)
                .map(super::media::thumbnail_key)
                .collect()
        } else {
            self.timeline
                .iter()
                .filter(|m| {
                    m.kind == "sticker"
                        && self
                            .visible_media
                            .iter()
                            .any(|(sender, id)| *sender == m.sender && *id == m.id)
                })
                .map(super::media::thumbnail_key)
                .collect()
        };
        self.animations.activate(keys);
    }
}
