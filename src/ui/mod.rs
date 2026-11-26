mod actions;
mod animation;
mod attachments;
mod audio;
mod background;
mod calling;
mod context_menu;
mod details;
mod diagnostics;
mod fonts;
mod formatting;
mod links;
mod locale;
mod mcp;
mod media;
mod mentions;
mod phone;
mod picker;
mod preferences;
mod qr;
mod recording;
mod render;
mod search;
mod sidebar;
mod startup;
mod stories;
mod times;
mod updates;
mod view;

use crate::playback as video;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, mpsc},
    time::Duration,
};

use anyhow::Result;
use rune::{App, AppSpec, Image, ImageCache, TextEdit, Tx, Waker};
use serde_json::{Value, json};

use crate::{
    assets::AssetRequest,
    config::Config,
    ipc,
    model::{
        Chat, MediaPage, Message, MessageCursor, MessagePage, SearchHit, SearchResults, Snapshot,
    },
};

static IMAGE_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static IMAGE_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static MESSAGE_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static VIDEO_STATUS: std::sync::Mutex<Option<video::Status>> = std::sync::Mutex::new(None);
static VOICE_STATUS: std::sync::Mutex<Option<video::Status>> = std::sync::Mutex::new(None);

const SNAPSHOT_LOAD_ERROR: &str =
    "The local service is taking too long to load your conversations. Try again.";

pub(crate) fn memory_report() -> Value {
    use std::sync::atomic::Ordering::Relaxed;
    json!({"renderer":render::report(),"decoded_image_estimate_bytes":IMAGE_BYTES.load(Relaxed),"image_count":IMAGE_COUNT.load(Relaxed),"timeline_messages":MESSAGE_COUNT.load(Relaxed),"video":VIDEO_STATUS.lock().unwrap_or_else(|e|e.into_inner()).clone(),"voice":VOICE_STATUS.lock().unwrap_or_else(|e|e.into_inner()).clone(),"sticker_animations":animation::report()})
}

#[derive(Default)]
struct UiWake {
    waker: Option<Waker>,
    app: Option<rune::AppHandle<State>>,
}

enum Command {
    Deselect,
    ManageChat(&'static str, Value),
    /// A sticker from the picker for a chat: its upload runs beside other work.
    SendSticker(String, String),
    CopyMedia(u64, Message),
    /// Sticker work that takes seconds: the tool, its arguments and what to say when done.
    Sticker(&'static str, Value, &'static str),
    CopyProfile(u64, String),
    SaveProfile(String, std::path::PathBuf),
    ReplyStatus(String, String, String, String),
    ViewStatus(String, String),
    NextAudio(u64, Message),
    Start,
    Refresh,
    Update,
    Call(&'static str, Value),
    Search(search::Request),
    Members(String),
    Stickers(bool),
    RememberEmoji(String),
    Jump(Message, u64),
    Gallery(String, String, Option<MessageCursor>),
    Prefetch(String, Vec<(String, String)>),
    Asset(String, AssetRequest),
    Video(u64, Message),
    Audio(u64, Message),
    /// Voice messages on screen to transcribe: generation (shared, so a newer
    /// request stops older polling), chat, messages, messages to retry and
    /// whether those are transcribed again even when finished.
    Transcribe(
        Arc<std::sync::atomic::AtomicU64>,
        u64,
        String,
        Vec<(String, String)>,
        Vec<(String, String)>,
        bool,
    ),
    SendVoice(u64, crate::model::VoiceUpload),
    PrepareAttachment(u64, u64, attachments::Source),
    SendAttachments(u64, String, Vec<attachments::Upload>),
    Preferences(&'static str, Value),
    Diagnostics,
    Select(String, u64),
    Viewing(Option<String>),
    Focused(bool),
    Tool(&'static str, Value),
    Send(Message, Option<MessageCursor>),
    /// Edit one of your messages: the message as shown before and its new text.
    Edit(Message, String),
    Forward(String, String, Vec<MessageCursor>),
    Open(String),
    Older(String, MessageCursor, u64),
    Newer(String, MessageCursor, u64),
}
enum Event {
    MediaCopied(u64, Result<rune::ClipboardContent, String>),
    StickerDone(Result<&'static str, String>),
    StatusReplied(String, Result<Message, String>),
    NextAudio(u64, Result<Option<Message>, String>),
    Activate(Option<String>),
    Call(Result<Value, String>),
    CallFrame(
        String,
        bool,
        u32,
        u32,
        Vec<u8>,
        Arc<std::sync::atomic::AtomicBool>,
    ),
    Snapshot(Snapshot),
    Conversation(String, u64, Result<MessagePage, String>),
    SnapshotFailed(String),
    Update(Result<crate::updates::UpdateState, String>),
    Search(search::Request, Result<SearchResults, String>),
    Members(String, Result<Vec<crate::model::Member>, String>),
    Stickers(bool, Result<Value, String>),
    Jump(Message, u64, Result<MessagePage, String>),
    Gallery(String, String, Result<MediaPage, String>),
    Asset(String, Result<Option<Vec<u8>>, String>),
    Video(u64, Result<tempfile::TempPath, String>),
    Audio(u64, Result<tempfile::TempPath, String>),
    Transcripts(String, Vec<crate::transcription::Item>),
    VoiceSent(u64, Result<Message, String>),
    AttachmentPrepared(u64, u64, Result<attachments::Prepared, String>),
    AttachmentSent(u64, u64, Result<Message, String>),
    AttachmentsFinished(u64),
    Preferences(&'static str, Result<Value, String>),
    Diagnostics(Result<Value, String>),
    Done(Result<Value, String>),
    Sent(String, Result<Message, String>),
    Edited(Message, Result<Message, String>),
    Forward(Result<crate::model::ForwardResult, String>),
    Opened(Result<(String, Option<String>), String>),
    Older(String, u64, Result<MessagePage, String>),
    Newer(String, u64, Result<MessagePage, String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Messages,
    Calls,
    Status,
}

struct PairingQr {
    image: Image,
    size: [u32; 2],
    logo: Option<qr::Logo>,
}

pub(super) struct State {
    context_menu: Option<context_menu::Open>,
    delete_chat: Option<Chat>,
    viewport: [f32; 2],
    copy_generation: u64,
    story: Option<stories::Playback>,
    background: background::Background,
    config: Config,
    preferences: preferences::Panel,
    picker: picker::Panel,
    contact: Option<Message>,
    typing_text: String,
    typing_sent_at: i64,
    window: Arc<std::sync::Mutex<Option<rune::WindowHandle>>>,
    reply: Option<Message>,
    /// The message being edited in the composer, and the draft it replaced.
    editing: Option<(Message, String)>,
    reaction_menu: Option<Message>,
    reaction_text: TextEdit,
    forwarding: Option<actions::Forwarding>,
    voice: Option<audio::Voice>,
    voice_generation: u64,
    voice_next_tick: Option<f64>,
    voice_rate: f64,
    call: calling::Panel,
    floating: Option<calling::Floating>,
    recording: Option<recording::Panel>,
    recording_generation: u64,
    attachments: Option<attachments::Panel>,
    attachment_drag_over: bool,
    attachment_generation: u64,
    attachment_sequence: u64,
    diagnostics: diagnostics::Panel,
    section: Section,
    sidebar: rune_kit::shell::PaneState,
    search: search::Panel,
    chat_search: search::Panel,
    composer_focus_at: Option<std::time::Instant>,
    search_focus_at: Option<std::time::Instant>,
    history_focus: bool,
    highlighted: Option<String>,
    history_notice: Option<String>,
    attachments_open: bool,
    viewer: Option<Message>,
    viewer_zoom: media::Zoom,
    chat_bounds: [f32; 4],
    viewer_generation: u64,
    video: Option<video::Player>,
    video_status: Option<video::Status>,
    video_next_tick: Option<f64>,
    video_error: Option<String>,
    avatars: HashMap<String, Image>,
    avatar_versions: HashMap<String, i64>,
    images: HashMap<String, Image>,
    animations: animation::Player,
    image_sizes: HashMap<String, [u32; 2]>,
    image_order: VecDeque<String>,
    image_pending: HashSet<String>,
    image_failures: HashMap<String, (i64, Option<String>)>,
    logo: Option<Image>,
    startup_art: Vec<Image>,
    startup_error: Option<String>,
    snapshot: Snapshot,
    selected: Option<String>,
    draft: TextEdit,
    phone: TextEdit,
    phone_country: phone::Picker,
    drafts: HashMap<String, String>,
    pending_sends: HashMap<String, (Message, Option<MessageCursor>)>,
    /// Each group's members for `@` mentions, asked for once per session.
    members: HashMap<String, Vec<crate::model::Member>>,
    members_pending: HashSet<String>,
    /// The highlighted member while typing a mention, the mention it belongs to,
    /// and one closed with Esc (by its `@` position).
    mention_index: usize,
    mention_seen: Option<(usize, String)>,
    mention_dismissed: Option<usize>,
    /// The `@Name` tags picked in each chat's draft and whom they name.
    draft_mentions: HashMap<String, Vec<(String, crate::model::Member)>>,
    /// The sticker whose sound is on (chat, sender, ID): it plays with the
    /// animation and both start over together.
    sticker_sound: Option<(String, String, String)>,
    phone_mode: bool,
    details: bool,
    gallery: Vec<Message>,
    gallery_kind: String,
    gallery_next: Option<MessageCursor>,
    gallery_loading: bool,
    media_menu: Option<(String, Message)>,
    detached: bool,
    show_archived: bool,
    chat_kind: sidebar::ChatKind,
    /// The kind the open sidebar search is filtered to; it starts at All.
    search_kind: sidebar::ChatKind,
    /// The kind chips' sideways scroll: offset and furthest offset.
    chip_scroll: rune::Live<[f32; 2]>,
    /// How strongly each edge arrow of the chips shows (0 when nothing is hidden past it).
    chip_edges: [rune::Live<f32>; 2],
    badge: Option<u32>,
    /// A WhatsApp link opened from outside, waiting for the connection.
    pending_link: Option<String>,
    /// A short confirmation shown at the bottom, and when it disappears.
    notice: Option<(String, Option<f64>)>,
    sticker_jobs: usize,
    visible_media: Vec<(String, String)>,
    prefetched: HashSet<String>,
    /// Latest known transcripts, by `transcript_key`, overriding loaded messages.
    transcripts: HashMap<String, crate::transcription::Transcript>,
    /// Transcripts shown in full instead of their beginning.
    transcripts_open: HashSet<String>,
    /// The on-screen voice messages last sent for transcription.
    transcript_request: Vec<(String, String)>,
    transcript_generation: Arc<std::sync::atomic::AtomicU64>,
    scroll_height: std::rc::Rc<std::cell::Cell<f32>>,
    scroll_thumb: rune::Live<[f32; 4]>,
    scroll_metrics: rune::Live<[f32; 2]>,
    busy: bool,
    authenticated: bool,
    focused: bool,
    viewing_sent: Option<String>,
    second: i64,
    next_tick: Option<f64>,
    timeline: Vec<crate::model::Message>,
    loading_chat: bool,
    loading_older: bool,
    has_older: bool,
    has_newer: bool,
    loading_newer: bool,
    history_generation: u64,
    error: Option<String>,
    tx: tokio::sync::mpsc::UnboundedSender<Command>,
    rx: mpsc::Receiver<Event>,
    image_cache: Option<ImageCache>,
    qr: Option<PairingQr>,
    qr_payload: Option<String>,
    wake: Arc<std::sync::Mutex<UiWake>>,
}

pub fn run(config: Config, initial_chat: Option<String>) -> Result<()> {
    let window_title = if config.demo {
        "Brazap — Demo"
    } else {
        "Brazap"
    };
    #[cfg(target_os = "windows")]
    if !config.demo {
        crate::windows_integration::initialize(&config);
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let (events, incoming) = mpsc::channel();
    let wake = Arc::new(std::sync::Mutex::new(UiWake::default()));
    let thread_wake = wake.clone();
    let ui_config = config.clone();
    let worker = std::thread::Builder::new()
        .name("brazap-service".into())
        .spawn(move || {
            procwatch::alloc::tag_thread("service");
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .on_thread_start(|| procwatch::alloc::tag_thread("async workers"))
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => {
                    runtime.block_on(worker(config, rx, events, thread_wake, initial_chat))
                }
                Err(error) => {
                    log::error!("Could not start application runtime: {error}");
                }
            }
        })?;
    let start = tx.clone();
    // Rune's headless snapshots have no native on_ready callback. Demo mode
    // only opens the isolated archive, so it can populate those views now.
    if ui_config.demo {
        let _ = start.send(Command::Start);
    }
    let window = Arc::new(std::sync::Mutex::new(None));
    let window_created = window.clone();
    let mut state = State {
        context_menu: None,
        delete_chat: None,
        viewport: [1180.0, 800.0],
        copy_generation: 0,
        background: Default::default(),
        config: ui_config,
        preferences: Default::default(),
        picker: Default::default(),
        contact: None,
        typing_text: String::new(),
        typing_sent_at: 0,
        window,
        reply: None,
        editing: None,
        reaction_menu: None,
        reaction_text: TextEdit::new(""),
        forwarding: None,
        voice: None,
        voice_generation: 0,
        voice_next_tick: None,
        voice_rate: 1.0,
        call: Default::default(),
        floating: None,
        recording: None,
        recording_generation: 0,
        attachments: None,
        attachment_drag_over: false,
        attachment_generation: 0,
        attachment_sequence: 0,
        diagnostics: diagnostics::Panel::default(),
        section: Section::Messages,
        story: None,
        sidebar: rune_kit::shell::PaneState::new(360.0).range(320.0, 500.0),
        search: search::Panel::default(),
        chat_search: search::Panel::default(),
        composer_focus_at: None,
        search_focus_at: None,
        history_focus: false,
        highlighted: None,
        history_notice: None,
        attachments_open: false,
        viewer: None,
        viewer_zoom: media::Zoom::default(),
        chat_bounds: [333.0, 16.0, 831.0, 768.0],
        viewer_generation: 0,
        video: None,
        video_status: None,
        video_next_tick: None,
        video_error: None,
        avatars: HashMap::new(),
        avatar_versions: HashMap::new(),
        images: HashMap::new(),
        animations: Default::default(),
        image_sizes: HashMap::new(),
        image_order: VecDeque::new(),
        image_pending: HashSet::new(),
        image_failures: HashMap::new(),
        logo: None,
        startup_art: Vec::new(),
        startup_error: None,
        snapshot: Snapshot::default(),
        selected: None,
        draft: TextEdit::new(""),
        phone: TextEdit::new(""),
        phone_country: phone::Picker::default(),
        drafts: HashMap::new(),
        pending_sends: HashMap::new(),
        members: HashMap::new(),
        members_pending: HashSet::new(),
        mention_index: 0,
        mention_seen: None,
        mention_dismissed: None,
        draft_mentions: HashMap::new(),
        sticker_sound: None,
        phone_mode: false,
        details: false,
        gallery: Vec::new(),
        gallery_kind: "media".into(),
        gallery_next: None,
        gallery_loading: false,
        media_menu: None,
        detached: false,
        show_archived: false,
        chat_kind: sidebar::ChatKind::All,
        search_kind: sidebar::ChatKind::All,
        chip_scroll: rune::Live::new([0.0; 2]),
        chip_edges: [rune::Live::new(0.0), rune::Live::new(0.0)],
        badge: None,
        pending_link: None,
        notice: None,
        sticker_jobs: 0,
        visible_media: Vec::new(),
        prefetched: HashSet::new(),
        transcripts: HashMap::new(),
        transcripts_open: HashSet::new(),
        transcript_request: Vec::new(),
        transcript_generation: Default::default(),
        scroll_height: std::rc::Rc::new(std::cell::Cell::new(1.0)),
        scroll_thumb: rune::Live::new([0.0; 4]),
        scroll_metrics: rune::Live::new([0.0; 2]),
        busy: false,
        authenticated: false,
        focused: true,
        viewing_sent: None,
        second: crate::model::now(),
        next_tick: None,
        timeline: Vec::new(),
        loading_chat: false,
        loading_older: false,
        has_older: true,
        has_newer: false,
        loading_newer: false,
        history_generation: 0,
        error: None,
        tx,
        rx: incoming,
        image_cache: None,
        qr: None,
        qr_payload: None,
        wake,
    };
    let spec = AppSpec {
        fonts: fonts::registered(),
        families: (
            "Inter, 'Brazap Emoji'",
            "ui-monospace, Inter, 'Brazap Emoji'",
        ),
        min_size: Some((860.0, 620.0)),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new(window_title, (1180.0, 800.0), unreachable_state, view::view)
    };
    let mut app = App::new(spec)
        .maximized(true)
        // Bounded 4-8 MiB suballocation blocks keep the GPU memory a mostly idle chat window
        // reserves small. Buffers and textures still grow with their content.
        .gpu_memory_policy(rune::GpuMemoryPolicy::Manual {
            block_size: 4 * 1024 * 1024..8 * 1024 * 1024,
        })
        .on_focus(|s, focused, _| {
            s.focused = focused;
            let _ = s.tx.send(Command::Focused(focused));
            s.update_viewing();
            s.sync_floating();
            // Windows drops the overlay when the taskbar button is recreated after a hide.
            s.badge = None;
            s.update_badge();
        })
        .compositor(render::Renderer::default())
        .on_window(move |handle| {
            #[cfg(target_os = "windows")]
            crate::windows_integration::set_window_icon(handle);
            // The call window is created through this app too; keep the main one.
            if handle.key() != "floating-call"
                && let Ok(mut window) = window_created.lock()
            {
                *window = Some(handle.clone());
            }
        })
        .on_ready(move |_| {
            let _ = start.send(Command::Start);
        });
    state.background.attach(app.handle(), state.config.demo);
    if state.background.tray.is_some() {
        app = app
            .keep_running()
            .release_gpu_when_closed()
            .on_close(background::close);
    }
    if let Ok(mut wake) = state.wake.lock() {
        wake.app = Some(app.handle());
    }
    state.floating = Some(calling::Floating::attach(&mut app));
    app.init(move || state)
        .try_run()
        .map_err(|error| anyhow::anyhow!("Could not open the desktop window: {error}"))?;
    // Quit drops the last command sender and flushes the shared session. A tray
    // close only hides the window; its state and service stay alive.
    let _ = worker.join();
    Ok(())
}

fn transcript_key(chat: &str, sender: &str, id: &str) -> String {
    format!("{chat}\u{1f}{sender}\u{1f}{id}")
}

fn unreachable_state() -> State {
    unreachable!("App::init supplies the application state")
}

fn setup(state: &mut State, wake: Waker) {
    let cache = ImageCache::new(wake.clone());
    // The logo is displayed at 40 points; retaining its 1536-pixel source costs ~8 MiB.
    state.logo = image::load_from_memory(include_bytes!("../../assets/logo.png"))
        .ok()
        .map(|image| {
            let pixels = image
                .crop_imm(318, 64, 894, 894)
                .thumbnail(192, 192)
                .into_rgba8();
            cache.rgba(pixels.width(), pixels.height(), pixels.into_raw())
        });
    state.image_cache = Some(cache);
    startup::sync_art(state);
    state.load_phone_flags(false);
    if let Ok(mut slot) = state.wake.lock() {
        slot.waker = Some(wake);
    }
}

fn emit(sender: &mpsc::Sender<Event>, wake: &Arc<std::sync::Mutex<UiWake>>, event: Event) {
    let app = wake.lock().ok().and_then(|wake| wake.app.clone());
    if let Some(app) = app {
        match event {
            Event::Snapshot(snapshot) => {
                app.update_if(move |state, tx| {
                    let changed = receive_snapshot(state, snapshot, tx);
                    if changed {
                        state.sync_floating();
                    }
                    changed
                });
                return;
            }
            Event::Call(result) => {
                app.update(move |state, _| state.receive_call_result(result));
                return;
            }
            Event::CallFrame(id, local, width, height, rgba, pending) => {
                app.update_if(move |state, _| {
                    state.receive_call_frame(id, local, width, height, rgba, pending);
                    // Pixels wakes every window actually showing this stream.
                    false
                });
                return;
            }
            event => {
                if sender.send(event).is_ok()
                    && let Ok(wake) = wake.lock()
                    && let Some(waker) = &wake.waker
                {
                    waker();
                }
                return;
            }
        }
    }
    if sender.send(event).is_ok()
        && let Ok(wake) = wake.lock()
        && let Some(waker) = &wake.waker
    {
        waker();
    }
}

async fn worker(
    config: Config,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<Command>,
    events: mpsc::Sender<Event>,
    wake: Arc<std::sync::Mutex<UiWake>>,
    initial_chat: Option<String>,
) {
    // The window may report focus or other changes before it is ready; only Start begins.
    loop {
        match commands.recv().await {
            Some(Command::Start) => break,
            Some(_) => {}
            None => return,
        }
    }
    let instance = loop {
        match ipc::start_or_connect(config.clone()).await {
            Ok(instance) => break instance,
            Err(error) => {
                log::warn!("Could not start the local service: {error:#}");
                emit(
                    &events,
                    &wake,
                    Event::SnapshotFailed("Could not start the local service. Try again.".into()),
                );
                loop {
                    match commands.recv().await {
                        Some(Command::Start | Command::Refresh) => break,
                        Some(_) => {}
                        None => return,
                    }
                }
            }
        }
    };
    let remote = instance.remote.clone();
    if let Some(chat) = initial_chat {
        emit(&events, &wake, Event::Activate(Some(chat)));
    }
    let mut selected = None;
    let mut viewing = None;
    let mut focused = true;
    let attachment_slots = Arc::new(tokio::sync::Semaphore::new(1));
    let avatar_slots = Arc::new(tokio::sync::Semaphore::new(2));
    let media_slots = Arc::new(tokio::sync::Semaphore::new(4));
    let mut assets = tokio::task::JoinSet::new();
    let mut interval = tokio::time::interval(Duration::from_millis(700));
    // Resuming after sleep must not replay every missed polling tick.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut snapshot_retry_at = None;
    let mut snapshot_failures = 0u32;
    let mut snapshot_loaded = false;
    let (snapshot_done, mut snapshot_results) =
        tokio::sync::mpsc::unbounded_channel::<(u64, Option<String>, Result<Snapshot, String>)>();
    let mut snapshot_generation = 0u64;
    let mut snapshot_pending = false;
    let mut call_tick = tokio::time::interval(Duration::from_millis(100));
    let mut preview_call: Option<crate::calling::LiveCall> = None;
    let mut preview_id = String::new();
    let sequences = Arc::new(std::sync::Mutex::new([0u64; 2]));
    // At most one unconsumed preview per direction, including minimized windows.
    let frame_pending: [Arc<std::sync::atomic::AtomicBool>; 2] =
        std::array::from_fn(|_| Arc::new(std::sync::atomic::AtomicBool::new(false)));
    // Previews are fetched off this loop, so a slow service never delays chat
    // selection, sends or call controls behind a video frame.
    let frames_in_flight = Arc::new(std::sync::atomic::AtomicBool::new(false));
    loop {
        tokio::select! {
            command=commands.recv()=>{
                match command {
                    Some(Command::Deselect)=>{
                        selected=None;viewing=None;snapshot_generation=snapshot_generation.wrapping_add(1);
                        snapshot_retry_at=None;interval.reset_immediately();continue;
                    }
                    Some(Command::ManageChat(name,args))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call(name,args).await.map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Done(result));
                        });continue;
                    }
                    Some(Command::SendSticker(chat,hash))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("send_sticker",json!({"chat":chat,"hash":hash})).await
                                .and_then(|v|Ok(serde_json::from_value(v["message"].clone())?)).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Sent(String::new(),result));
                        });continue;
                    }
                    Some(Command::CopyMedia(generation,message))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();let config=config.clone();
                        assets.spawn(async move {
                            let result=context_menu::prepare_copy(&remote,&config,message).await.map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::MediaCopied(generation,result));
                        });continue;
                    }
                    Some(Command::Sticker(name,args,done))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call(name,args).await.map(|_|done).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::StickerDone(result));
                        });continue;
                    }
                    Some(Command::CopyProfile(generation,jid))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=context_menu::copy_profile(&remote,jid).await.map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::MediaCopied(generation,result));
                        });continue;
                    }
                    Some(Command::SaveProfile(jid,path))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=context_menu::save_profile(&remote,jid,path).await.map(|_|json!({"saved":true})).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Done(result));
                        });continue;
                    }
                    Some(Command::ReplyStatus(sender,id,text,message_id))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("reply_status",json!({"sender":sender,"id":id,"text":text,"message_id":message_id})).await
                                .and_then(|v|Ok(serde_json::from_value(v["message"].clone())?)).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::StatusReplied(message_id,result));
                        });continue;
                    }
                    Some(Command::ViewStatus(sender,id))=>{
                        let remote=remote.clone();
                        assets.spawn(async move {
                            if let Err(error)=remote.call("view_status",json!({"sender":sender,"id":id})).await { log::debug!("Could not mark status viewed: {error}"); }
                        });continue;
                    }
                    Some(Command::NextAudio(generation,message))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("next_audio",json!({"chat":message.chat,"cursor":message.cursor()})).await
                                .and_then(|v|Ok(serde_json::from_value(v["message"].clone())?)).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::NextAudio(generation,result));
                        });continue;
                    }
                    None=>break,
                    Some(Command::Start)=>continue,
                    Some(Command::Refresh)=>{
                        snapshot_retry_at=None;
                        interval.reset_immediately();
                        continue;
                    }
                    Some(Command::Update)=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.request("brazap/updates",json!({"download":true})).await
                                .and_then(|v|Ok(serde_json::from_value(v)?)).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Update(result));
                        });continue;
                    }
                    Some(Command::Call(name,args))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call(name,args).await.map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Call(result));
                        });continue;
                    }
                    Some(Command::Gallery(chat,kind,cursor))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("chat_media",json!({"chat":chat,"kind":kind,"cursor":cursor})).await.and_then(|v|Ok(serde_json::from_value(v)?)).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Gallery(chat,kind,result));
                        });continue;
                    }
                    Some(Command::Transcribe(current,generation,chat,items,retry,redo))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            use std::sync::atomic::Ordering::Relaxed;
                            let mut pending=items;
                            let mut retry=retry;
                            // Poll until every message is finished or a newer view replaces this one.
                            while current.load(Relaxed)==generation && !pending.is_empty() {
                                match remote.call("transcribe",json!({"chat":chat,"items":pending,"visible":true,"retry":retry,"redo":redo && !retry.is_empty()})).await
                                    .and_then(|v|Ok(serde_json::from_value::<Vec<crate::transcription::Item>>(v["items"].clone())?)) {
                                    Ok(results)=>{
                                        pending=results.iter().filter(|item|item.transcript.as_ref().is_some_and(|t|!t.finished())).map(|item|(item.sender.clone(),item.id.clone())).collect();
                                        if current.load(Relaxed)==generation {
                                            emit(&events,&wake,Event::Transcripts(chat.clone(),results));
                                        }
                                    }
                                    Err(error)=>{
                                        log::debug!("Could not request transcripts: {error:#}");
                                        break;
                                    }
                                }
                                retry=Vec::new();
                                tokio::time::sleep(Duration::from_millis(1200)).await;
                            }
                        });continue;
                    }
                    Some(Command::Prefetch(chat,items))=>{
                        let remote=remote.clone();
                        assets.spawn(async move {if let Err(error)=remote.call("prepare_media",json!({"chat":chat,"items":items})).await{log::debug!("Could not prepare media: {error}");}});
                        continue;
                    }
                    Some(Command::Members(chat))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("group_members",json!({"chat":chat})).await.and_then(|v|Ok(serde_json::from_value(v)?)).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Members(chat,result));
                        });continue;
                    }
                    Some(Command::Search(request))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("search",json!({"query":request.query,"chat":request.chat,"cursor":request.cursor,"kind":request.kind})).await.and_then(|v|Ok(serde_json::from_value(v)?)).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Search(request,result));
                        });continue;
                    }
                    Some(Command::Jump(message, generation))=>{
                        selected=Some(message.chat.clone());
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("message_window",json!({"chat":message.chat,"sender":message.sender,"id":message.id})).await.and_then(|v|Ok(serde_json::from_value(v)?)).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Jump(message,generation,result));
                        });continue;
                    }
                    Some(Command::Stickers(sync))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {let result=remote.call("sticker_catalog",json!({"sync":sync})).await.map_err(|e|format!("{e:#}"));emit(&events,&wake,Event::Stickers(sync,result));});continue;
                    }
                    Some(Command::RememberEmoji(emoji))=>{
                        let remote=remote.clone();assets.spawn(async move {let _=remote.call("remember_emoji",json!({"emoji":emoji})).await;});continue;
                    }
                    Some(Command::Asset(key,request))=>{
                        let slots=if matches!(&request,AssetRequest::Avatar{..}|AssetRequest::Profile{..}) {avatar_slots.clone()} else {media_slots.clone()};
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let _permit=slots.acquire_owned().await;
                            let result=remote.asset(request).await.map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Asset(key,result));
                        });
                        continue;
                    },
                    Some(Command::Video(generation,message))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=async {
                                let file=tempfile::Builder::new().prefix("brazap-video-").suffix(".mp4").tempfile()?.into_temp_path();
                                remote.call("save_attachment",json!({"chat":message.chat,"sender":message.sender,"id":message.id,"path":file.to_string_lossy()})).await?;
                                Ok::<_,anyhow::Error>(file)
                            }.await.map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Video(generation,result));
                        });continue;
                    }
                    Some(Command::Audio(generation,message))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=async {
                                let suffix=match message.mime.as_deref(){Some("audio/mpeg")=>".mp3",Some("audio/mp4")=>".m4a",Some("audio/wav")=>".wav",_=>".ogg"};
                                let file=tempfile::Builder::new().prefix("brazap-audio-").suffix(suffix).tempfile()?.into_temp_path();
                                // A sticker with sound plays the audio inside the sticker.
                                let method=if message.kind=="sticker" {"save_sticker_sound"} else {"save_attachment"};
                                remote.call(method,json!({"chat":message.chat,"sender":message.sender,"id":message.id,"path":file.to_string_lossy()})).await?;
                                tokio::task::spawn_blocking(move||crate::playback::prepare_audio(file)).await?
                            }.await.map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Audio(generation,result));
                        });continue;
                    }
                    Some(Command::PrepareAttachment(generation,id,source))=>{
                        let events=events.clone();let wake=wake.clone();let slots=attachment_slots.clone();
                        assets.spawn(async move {
                            let _permit=slots.acquire_owned().await;
                            let result=attachments::prepare(source).await.map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::AttachmentPrepared(generation,id,result));
                        });continue;
                    }
                    Some(Command::SendAttachments(generation,chat,uploads))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            for upload in uploads {
                                let result=remote.call("send_attachment",json!({"chat":chat,"path":upload.path,"kind":upload.kind,"caption":upload.caption,"view_once":upload.view_once,"mentions":upload.mentions,"mention_everyone":upload.mention_everyone})).await
                                    .and_then(|value|Ok(serde_json::from_value(value["message"].clone())?))
                                    .map_err(|e|format!("{e:#}"));
                                let failed=result.is_err();
                                emit(&events,&wake,Event::AttachmentSent(generation,upload.id,result));
                                if failed { break; }
                            }
                            emit(&events,&wake,Event::AttachmentsFinished(generation));
                        });continue;
                    }
                    Some(Command::SendVoice(generation,upload))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("send_voice",json!(upload)).await
                                .and_then(|v|Ok(serde_json::from_value(v["message"].clone())?))
                                .map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::VoiceSent(generation,result));
                        });continue;
                    }
                    Some(Command::Preferences(action,args))=>{
                        if action=="logout" {
                            snapshot_generation=snapshot_generation.wrapping_add(1);
                            selected=None;
                            viewing=None;
                            preview_call=None;
                            snapshot_retry_at=None;
                            interval.reset_immediately();
                        }
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let method=match action {"get"=>"brazap/settings","save"=>"brazap/settings/save","privacy"=>"brazap/privacy","privacy_set"=>"brazap/privacy/set","clear"=>"brazap/cache/clear","logout"=>"brazap/logout","typing"=>"brazap/typing","speech_model"=>"brazap/call",_=>"brazap/updates"};
                            let mut result=remote.request(method,args).await.map_err(|e|format!("{e:#}"));
                            if action=="privacy_set" && result.is_ok(){result=remote.request("brazap/privacy",json!({})).await.map_err(|e|format!("{e:#}"));}
                            if action!="typing" {emit(&events,&wake,Event::Preferences(action,result));}
                        });continue;
                    }
                    Some(Command::Diagnostics)=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.request("brazap/diagnostics",json!({})).await.map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Diagnostics(result));
                        });continue;
                    }
                    Some(Command::Viewing(chat))=>{viewing=chat;}
                    Some(Command::Focused(now))=>{focused=now;interval.reset_immediately();}
                    Some(Command::Select(chat,generation))=>{
                        selected=Some(chat.clone());
                        snapshot_retry_at=None;
                        let reader=remote.clone();let loaded=events.clone();let ready=wake.clone();let requested=chat.clone();
                        assets.spawn(async move {
                            let result=reader.call("read_messages",json!({"chat":requested,"limit":crate::history::CHAT_PAGE_SIZE})).await
                                .and_then(|v|Ok(serde_json::from_value(v)?)).map_err(|e|format!("{e:#}"));
                            emit(&loaded,&ready,Event::Conversation(requested,generation,result));
                        });
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            if let Err(error)=remote.call("mark_read",json!({"chat":chat})).await {
                                emit(&events,&wake,Event::Done(Err(format!("Could not mark the chat read: {error}"))));
                            }
                        });
                    }
                    // Nothing here waits on the service: a slow call never holds up the
                    // snapshot, images or other commands behind it.
                    Some(Command::Tool(name,args))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call(name,args).await.map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Done(result));
                        });continue;
                    }
                    Some(Command::Send(message,reply))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("send_message",json!({"chat":message.chat,"text":message.text,"reply_to":reply,"message_id":message.id,"timestamp":message.timestamp,"mentions":message.mentions.iter().map(|m|m.jid.as_str()).collect::<Vec<_>>(),"mention_everyone":message.mentions_everyone})).await
                                .and_then(|v|Ok(serde_json::from_value(v["message"].clone())?)).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Sent(message.id,result));
                        });
                    }
                    Some(Command::Edit(message,text))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("edit_message",json!({"chat":message.chat,"id":message.id,"text":text})).await
                                .and_then(|v|Ok(serde_json::from_value(v["message"].clone())?)).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Edited(message,result));
                        });
                    }
                    Some(Command::Forward(source,destination,targets))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("forward_messages",json!({"source":source,"destination":destination,"targets":targets})).await.and_then(|v|Ok(serde_json::from_value(v)?)).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Forward(result));
                        });continue;
                    }
                    // The opened chat comes back as a selection, which sets `selected`.
                    Some(Command::Open(address))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("open_chat",json!({"address":address})).await.and_then(|value|{
                                let chat=value["chat"].as_str().map(str::to_owned).ok_or_else(||anyhow::anyhow!("Missing chat address"))?;
                                Ok((chat,value["text"].as_str().map(str::to_owned)))
                            }).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Opened(result));
                        });continue;
                    }
                    Some(Command::Newer(chat,cursor,generation))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("newer_messages",json!({"chat":chat,"cursor":cursor,"limit":crate::history::CHAT_PAGE_SIZE})).await
                                .and_then(|value|Ok(serde_json::from_value(value)?)).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Newer(chat,generation,result));
                        });continue;
                    }
                    Some(Command::Older(chat,cursor,generation))=>{
                        let remote=remote.clone();let events=events.clone();let wake=wake.clone();
                        assets.spawn(async move {
                            let result=remote.call("older_messages",json!({"chat":chat,"cursor":cursor})).await
                                .and_then(|value|Ok(serde_json::from_value(value)?)).map_err(|e|format!("{e:#}"));
                            emit(&events,&wake,Event::Older(chat,generation,result));
                        });continue;
                    }
                }
                interval.reset_immediately();
            }
            Some(_)=assets.join_next(), if !assets.is_empty()=>{},
            Some((generation,requested,result))=snapshot_results.recv()=>{
                snapshot_pending=false;
                if generation != snapshot_generation || requested != selected { interval.reset_immediately();continue; }
                match result {
                    Ok(snapshot)=>{
                        snapshot_retry_at=None;
                        snapshot_failures=0;
                        snapshot_loaded=true;
                        preview_call=snapshot.live_call.clone();
                        emit(&events,&wake,Event::Snapshot(snapshot));
                    },
                    Err(error)=>{
                        log::warn!("Could not load the local snapshot: {error}");
                        snapshot_failures=snapshot_failures.saturating_add(1);
                        // A request spanning suspend can expire as soon as the
                        // machine wakes. Keep the cached conversation and give
                        // a fresh request a chance before showing an error.
                        let delay=if snapshot_failures==1 {1} else {10};
                        snapshot_retry_at=Some(tokio::time::Instant::now()+Duration::from_secs(delay));
                        if !snapshot_loaded || snapshot_failures>1 {
                            emit(&events,&wake,Event::SnapshotFailed(SNAPSHOT_LOAD_ERROR.into()));
                        }
                    },
                }
            },
            _=call_tick.tick(), if preview_call.as_ref().is_some_and(|c| !c.ended() && (c.camera || c.peer_camera))=>{
                use std::sync::atomic::Ordering;
                let Some(call)=preview_call.clone() else { continue };
                if preview_id != call.id {
                    preview_id=call.id.clone();
                    *sequences.lock().unwrap_or_else(|e|e.into_inner())=[0,0];
                }
                if frames_in_flight.swap(true,Ordering::AcqRel) { continue; }
                let (remote,events,wake,sequences,frame_pending,in_flight)=(remote.clone(),events.clone(),wake.clone(),sequences.clone(),frame_pending.clone(),frames_in_flight.clone());
                assets.spawn(async move {
                    for (index,local) in [false,true].into_iter().enumerate() {
                        if frame_pending[index].load(Ordering::Acquire) { continue; }
                        let after=sequences.lock().unwrap_or_else(|e|e.into_inner())[index];
                        let Ok(value)=remote.request("brazap/call/frame",json!({"id":call.id,"local":local,"after":after})).await else { continue };
                        let Ok(Some(frame))=serde_json::from_value::<Option<crate::calling::PreviewFrame>>(value["frame"].clone()) else { continue };
                        // Sequences are process-wide, so a reply that outlives its call is harmless.
                        sequences.lock().unwrap_or_else(|e|e.into_inner())[index]=frame.sequence;
                        use base64::Engine;
                        let Ok(bytes)=base64::engine::general_purpose::STANDARD.decode(frame.jpeg) else { continue };
                        let Ok(image)=image::load_from_memory(&bytes) else { continue };
                        let mut image=image.to_rgba8();
                        if !local {
                            image=match calling::remote_rotation() {
                                1=>image::imageops::rotate90(&image),
                                2=>image::imageops::rotate180(&image),
                                3=>image::imageops::rotate270(&image),
                                _=>image,
                            };
                        }
                        frame_pending[index].store(true,Ordering::Release);
                        emit(&events,&wake,Event::CallFrame(call.id.clone(),local,image.width(),image.height(),image.into_raw(),frame_pending[index].clone()));
                    }
                    in_flight.store(false,Ordering::Release);
                });
            },
            _=interval.tick()=>{
                if snapshot_pending || snapshot_retry_at.is_some_and(|at| tokio::time::Instant::now() < at) { continue; }
                snapshot_pending=true;
                let generation=snapshot_generation;
                let remote=remote.clone();let done=snapshot_done.clone();let requested=selected.clone();let viewing=viewing.clone();let events=events.clone();let wake=wake.clone();
                // Keep the worker available for chat selection and page loads
                // while the complete sidebar snapshot is being assembled.
                assets.spawn(async move {
                    if let Ok(Ok(activation)) = tokio::time::timeout(Duration::from_secs(3), remote.request("brazap/desktop",json!({"pid":std::process::id(),"chat":viewing,"focused":focused}))).await
                        && activation["activate"].as_bool() == Some(true) {
                        emit(&events,&wake,Event::Activate(activation["chat"].as_str().map(str::to_owned)));
                    }
                    let result=match tokio::time::timeout(Duration::from_secs(15),remote.snapshot(requested.as_deref())).await {
                        Ok(result)=>result.map_err(|error|format!("{error:#}")),
                        Err(error)=>Err(error.to_string()),
                    };
                    let _=done.send((generation,requested,result));
                });
            }
        }
    }
    assets.abort_all();
    while assets.join_next().await.is_some() {}
    if let Some(service) = instance.owned {
        service.shutdown().await;
    }
}

fn poll(state: &mut State, elapsed: f64, tx: &mut Tx) -> bool {
    locale::configure(&state.preferences.value.appearance);
    state.poll_mentions();
    // Notices stay for a few seconds once shown; "Creating sticker…" stays until it is done.
    if state.sticker_jobs == 0
        && let Some((_, expiry)) = &mut state.notice
    {
        match *expiry {
            None => {
                *expiry = Some(elapsed + 3.0);
                tx.wake_after(Duration::from_secs(3));
            }
            Some(at) if elapsed >= at => state.notice = None,
            Some(_) => {}
        }
    }
    state
        .background
        .localize(&state.preferences.value.appearance.language);
    use std::sync::atomic::Ordering::Relaxed;
    IMAGE_BYTES.store(
        state
            .images
            .values()
            .chain(state.avatars.values())
            .chain(state.logo.iter())
            .chain(state.startup_art.iter())
            .chain(state.picker.images.values())
            .chain(
                state
                    .attachments
                    .iter()
                    .flat_map(|p| p.items.iter())
                    .filter_map(|i| i.image.as_ref()),
            )
            .filter_map(|image| image.size())
            .map(|[w, h]| u64::from(w) * u64::from(h) * 16 / 3)
            .sum::<u64>()
            + animation::report()["estimated_bytes"].as_u64().unwrap_or(0),
        Relaxed,
    );
    IMAGE_COUNT.store(
        state.images.len()
            + state.avatars.len()
            + state.startup_art.len()
            + state.picker.images.len()
            + state
                .attachments
                .iter()
                .flat_map(|p| p.items.iter())
                .filter(|i| i.image.is_some())
                .count(),
        Relaxed,
    );
    MESSAGE_COUNT.store(state.timeline.len(), Relaxed);
    crate::diagnostics::IMAGE_TEXTURES.set(IMAGE_BYTES.load(Relaxed) as usize);
    let mut changed = false;
    while let Ok(event) = state.rx.try_recv() {
        match event {
            Event::MediaCopied(generation, result) => {
                if generation == state.copy_generation {
                    match result {
                        Ok(content) => tx.copy_rich(content),
                        Err(error) => {
                            log::warn!("Could not copy media: {error}");
                            state.error = Some("Could not copy media. Try again.".into());
                        }
                    }
                    changed = true;
                }
            }
            Event::StickerDone(result) => {
                state.sticker_jobs = state.sticker_jobs.saturating_sub(1);
                match result {
                    Ok(done) => state.notice = Some((done.into(), None)),
                    Err(error) => {
                        state.notice = None;
                        state.error = Some(error);
                    }
                }
                changed = true;
            }
            Event::StatusReplied(id, result) => {
                state.receive_status_reply(id, result);
                changed = true;
            }
            Event::Transcripts(chat, items) => {
                for item in items {
                    if let Some(transcript) = item.transcript {
                        state
                            .transcripts
                            .insert(transcript_key(&chat, &item.sender, &item.id), transcript);
                    }
                }
                changed = true;
            }
            Event::NextAudio(generation, result) => {
                if generation == state.voice_generation && state.story.is_none() {
                    match result {
                        Ok(Some(message)) if state.selected.as_deref() == Some(&message.chat) => {
                            state.play_voice(message)
                        }
                        Err(error) => log::debug!("Could not load next audio message: {error}"),
                        _ => {}
                    }
                    changed = true;
                }
            }
            Event::AttachmentPrepared(generation, id, result) => {
                state.receive_attachment(generation, id, result);
                changed = true;
            }
            Event::AttachmentSent(generation, id, result) => {
                state.receive_attachment_sent(generation, id, result, tx);
                changed = true;
            }
            Event::AttachmentsFinished(generation) => {
                state.finish_attachments(generation);
                changed = true;
            }
            Event::VoiceSent(generation, result) => {
                state.receive_recording(generation, result, tx);
                changed = true;
            }
            Event::Call(result) => {
                state.receive_call_result(result);
                changed = true;
            }
            Event::CallFrame(id, local, width, height, rgba, pending) => {
                changed |= state.receive_call_frame(id, local, width, height, rgba, pending);
            }
            Event::Activate(chat) => {
                match chat {
                    // A WhatsApp link opened from the browser or another app.
                    Some(link) if link.starts_with("whatsapp:") => {
                        state.pending_link = Some(link);
                        state.open_pending_link();
                    }
                    Some(chat) => state.select(chat),
                    None => {}
                }
                if let Ok(window) = state.window.lock()
                    && let Some(window) = window.as_ref()
                {
                    window.open();
                }
                changed = true;
            }
            Event::Preferences(action, result) => {
                state.receive_preferences(action, result);
                changed = true;
            }
            Event::Diagnostics(result) => {
                state.diagnostics.receive(result);
                changed = true;
            }
            Event::Snapshot(snapshot) => {
                changed |= receive_snapshot(state, snapshot, tx);
            }
            Event::Conversation(chat, generation, result) => {
                if state.selected.as_deref() == Some(&chat)
                    && state.history_generation == generation
                    && state.loading_chat
                    && !state.history_focus
                {
                    match result {
                        Ok(page) => {
                            state.loading_chat = false;
                            state.has_older = page.has_older;
                            state.history_notice = page.notice;
                            state.merge_messages(page.messages);
                            state.request_chat_media();
                            tx.instant();
                            tx.scroll_to_bottom_instant("messages");
                        }
                        Err(error) => {
                            log::warn!("Could not load conversation: {error}");
                            state.history_notice =
                                Some("Could not load this conversation. Retrying…".into());
                        }
                    }
                    changed = true;
                }
            }
            Event::SnapshotFailed(error) => {
                if state.authenticated || state.qr.is_some() {
                    state.error = Some(error);
                } else {
                    state.startup_error = Some(error);
                }
                changed = true;
            }
            Event::Update(result) => {
                match result {
                    Ok(update) => state.snapshot.updates = update,
                    Err(error) => {
                        log::warn!("Could not download update: {error}");
                        state.snapshot.updates.status = "error".into();
                        state.error =
                            Some("Could not download update. It will retry automatically.".into());
                    }
                }
                changed = true;
            }
            Event::Gallery(chat, kind, result) => {
                if state.details
                    && state.selected.as_deref() == Some(&chat)
                    && state.gallery_kind == kind
                {
                    state.gallery_loading = false;
                    match result {
                        Ok(page) => {
                            state.gallery_next = page.next;
                            for item in page.items {
                                if !state
                                    .gallery
                                    .iter()
                                    .any(|m| m.id == item.id && m.sender == item.sender)
                                {
                                    state.gallery.push(item);
                                }
                            }
                            state.request_gallery_media();
                        }
                        Err(error) => state.error = Some(error),
                    }
                    changed = true;
                }
            }
            Event::Stickers(sync, result) => {
                state.receive_stickers(sync, result);
                changed = true;
            }
            Event::Search(request, result) => {
                let result = result.map(|mut results| {
                    results.chats.retain(|hit| {
                        state
                            .snapshot
                            .chats
                            .iter()
                            .any(|chat| chat.matches_jid(&hit.jid))
                    });
                    results.messages.retain(|hit| {
                        state
                            .snapshot
                            .chats
                            .iter()
                            .any(|chat| chat.matches_jid(&hit.chat.jid))
                    });
                    results
                });
                if request.chat.is_none() {
                    state.search.receive(&request, result);
                } else {
                    state.chat_search.receive(&request, result);
                }
                changed = true;
            }
            Event::Jump(message, generation, result) => {
                if state.selected.as_deref() == Some(&message.chat)
                    && state.highlighted.as_deref() == Some(&message.id)
                    && state.history_generation == generation
                {
                    state.loading_older = false;
                    match result {
                        Ok(page) => {
                            state.has_older = page.has_older;
                            state.has_newer = page.has_newer;
                            state.history_focus = page.has_newer;
                            state.timeline = page.messages;
                            state.request_chat_media();
                            let sender = state
                                .timeline
                                .iter()
                                .find(|m| {
                                    m.id == message.id
                                        && (m.sender == message.sender || message.sender.is_empty())
                                })
                                .or_else(|| state.timeline.iter().find(|m| m.id == message.id))
                                .map(|m| m.sender.as_str())
                                .unwrap_or(&message.sender);
                            tx.scroll_center_in(
                                "messages",
                                format!("message-{}-{}", sender, message.id),
                            );
                        }
                        Err(error) => {
                            state.history_focus = false;
                            state.error = Some(error);
                        }
                    }
                    changed = true;
                }
            }
            Event::Members(chat, result) => {
                state.members_pending.remove(&chat);
                match result {
                    Ok(members) => {
                        state.members.insert(chat, members);
                    }
                    Err(error) => log::debug!("Group members unavailable: {error}"),
                }
                changed = true;
            }
            Event::Asset(key, result) => {
                state.image_pending.remove(&key);
                match result {
                    Ok(Some(bytes)) => {
                        if let Some(cache) = &state.image_cache {
                            if let Some(identity) =
                                key.strip_prefix("avatar:").and_then(|k| k.rsplit_once(':'))
                            {
                                state
                                    .avatars
                                    .insert(identity.0.to_owned(), cache.load(bytes));
                                state
                                    .avatar_versions
                                    .insert(identity.0.to_owned(), identity.1.parse().unwrap_or(0));
                            } else {
                                // Header dimensions are available before Rune's async
                                // decoder uploads pixels, so layout gets the correct
                                // aspect ratio in the same state update as the asset.
                                if let Some((width, height)) =
                                    image::ImageReader::new(std::io::Cursor::new(&bytes))
                                        .with_guessed_format()
                                        .ok()
                                        .and_then(|r| r.into_dimensions().ok())
                                {
                                    state.image_sizes.insert(key.clone(), [width, height]);
                                }
                                let bytes: Arc<[u8]> = bytes.into();
                                if animation::is_animated(&bytes)
                                    && state.animations.load(key.clone(), bytes.clone())
                                {
                                    state.update_animations();
                                } else {
                                    state.images.insert(key.clone(), cache.load(bytes));
                                    state.image_order.retain(|k| k != &key);
                                    state.image_order.push_back(key);
                                }
                                state.trim_images();
                            }
                        }
                    }
                    Ok(None) => {
                        state
                            .image_failures
                            .insert(key, (crate::model::now(), None));
                    }
                    Err(error) => {
                        state
                            .image_failures
                            .insert(key, (crate::model::now(), Some(error)));
                    }
                }
                changed = true;
            }
            Event::Video(generation, result) => {
                if generation == state.viewer_generation && state.viewer.is_some() {
                    match result {
                        Ok(file) => {
                            let wake = state.wake.lock().ok().and_then(|w| w.waker.clone());
                            if let Some(wake) = wake {
                                state.video = Some(video::Player::new(file, wake));
                            }
                        }
                        Err(error) => state.video_error = Some(error),
                    }
                    changed = true;
                }
            }
            Event::Audio(generation, result) => {
                if generation == state.voice_generation
                    && let Some(voice) = &mut state.voice
                {
                    match result {
                        Ok(file) => {
                            if let Some(wake) = state.wake.lock().ok().and_then(|w| w.waker.clone())
                            {
                                let player = video::Player::new_audio(file, wake);
                                let sticker = voice.message.kind == "sticker";
                                // A sticker's sound keeps its own pace, from its first frame.
                                player.set_rate(if sticker { 1.0 } else { state.voice_rate });
                                voice.player = Some(player);
                                if sticker {
                                    let message = voice.message.clone();
                                    state.restart_sticker(&message);
                                }
                            }
                        }
                        Err(error) => voice.error = Some(error),
                    }
                    changed = true;
                }
            }
            Event::Done(result) => {
                state.busy = false;
                state.error = result.err();
                changed = true;
            }
            Event::Edited(original, result) => {
                let message = match result {
                    Ok(message) => message,
                    Err(error) => {
                        state.error = Some(error);
                        original
                    }
                };
                if state.selected.as_deref() == Some(&message.chat) {
                    state.merge_messages(vec![message]);
                }
                changed = true;
            }
            Event::Sent(id, result) => {
                match result {
                    Ok(message) => {
                        state.pending_sends.remove(&id);
                        if state
                            .selected
                            .as_ref()
                            .is_some_and(|chat| *chat == message.chat)
                        {
                            state.merge_messages(vec![message]);
                        }
                    }
                    Err(error) => {
                        if let Some((message, _)) = state.pending_sends.get_mut(&id) {
                            message.status = "failed".into();
                        }
                        if let Some(message) =
                            state.timeline.iter_mut().find(|m| m.id == id && m.from_me)
                            && !crate::outbox::confirmed(&message.status)
                        {
                            message.status = "failed".into();
                        }
                        state.error = Some(error);
                    }
                }
                changed = true;
            }
            Event::Forward(result) => {
                match result {
                    Ok(result) if result.error.is_none() => {
                        state.forwarding = None;
                        state.select(result.destination);
                        tx.scroll_to_bottom("messages");
                    }
                    Ok(result) => {
                        if let Some(flow) = &mut state.forwarding {
                            flow.sending = false;
                            flow.selected.retain(|m| {
                                !result
                                    .sent
                                    .iter()
                                    .any(|sent| sent.id == m.id && sent.sender == m.sender)
                            });
                            flow.error = result.error;
                        }
                    }
                    Err(error) => {
                        if let Some(flow) = &mut state.forwarding {
                            flow.sending = false;
                            flow.error = Some(error);
                        }
                    }
                }
                changed = true;
            }
            Event::Opened(result) => {
                state.busy = false;
                match result {
                    Ok((chat, text)) => {
                        state.close_search();
                        state.select(chat.clone());
                        // A link's message becomes the draft, without replacing one already typed.
                        if let Some(text) = text
                            && state.is_current_chat(&chat)
                            && state.draft.text.trim().is_empty()
                        {
                            state.draft = TextEdit::new(text);
                            state.draft.focus = state.draft.text.len();
                            state.draft.anchor = state.draft.focus;
                        }
                    }
                    Err(e) => state.error = Some(e),
                }
                changed = true;
            }
            Event::Newer(chat, generation, result) => {
                if state.selected.as_deref() == Some(&chat)
                    && state.history_generation == generation
                {
                    state.loading_newer = false;
                    match result {
                        Ok(page) => {
                            state.has_newer = page.has_newer;
                            state.history_focus = page.has_newer;
                            state.history_notice = None;
                            state.merge_messages(page.messages);
                            state.request_chat_media();
                        }
                        Err(error) => state.history_notice = Some(error),
                    }
                    changed = true;
                }
            }
            Event::Older(chat, generation, result) => {
                if state.selected.as_deref() == Some(&chat)
                    && state.history_generation == generation
                {
                    state.loading_older = false;
                    match result {
                        Ok(page) => {
                            state.has_older = page.has_older;
                            state.history_notice = page.notice;
                            state.merge_messages(page.messages);
                            state.request_chat_media();
                        }
                        Err(error) => state.error = Some(error),
                    }
                    changed = true;
                }
            }
        }
    }
    if state
        .snapshot
        .live_call
        .as_ref()
        .is_some_and(|c| !c.ended())
        && elapsed >= state.call.next_tick
    {
        state.call.next_tick = elapsed + 1.0;
        tx.wake_after(Duration::from_secs(1));
        changed = true;
    }
    changed |= startup::sync_art(state);
    state.update_viewing();
    changed |= state.update_picker_query(tx);
    state.update_animations();
    if state.diagnostics.open
        && !state.diagnostics.pending
        && elapsed >= state.diagnostics.next_sample
    {
        state.diagnostics.pending = true;
        state.diagnostics.next_sample = elapsed + 1.0;
        let _ = state.tx.send(Command::Diagnostics);
        tx.wake_after(Duration::from_secs(1));
    }
    // A sticker with its sound on loops: sound and animation start over together.
    let sticker_loop = state.voice.as_ref().and_then(|voice| {
        let player = voice.player.as_ref()?;
        let status = player.status();
        (status.ended
            && status.error.is_none()
            && !voice.last_status.as_ref().is_some_and(|s| s.ended)
            && state.sticker_sound_on(&voice.message))
        .then(|| voice.message.clone())
    });
    if let Some(message) = sticker_loop {
        if let Some(player) = state.voice.as_ref().and_then(|v| v.player.as_ref()) {
            player.seek(0.0);
            player.toggle();
        }
        state.restart_sticker(&message);
        if let Some(voice) = &mut state.voice {
            voice.last_status = None;
        }
    }
    if let Some(voice) = &mut state.voice
        && let Some(player) = &voice.player
    {
        let status = player.status();
        if status.ended
            && status.error.is_none()
            && !voice.last_status.as_ref().is_some_and(|s| s.ended)
            && state.story.is_none()
            // A sticker's sound plays once; it does not start the next voice message.
            && voice.message.kind != "sticker"
        {
            let _ = state.tx.send(Command::NextAudio(
                state.voice_generation,
                voice.message.clone(),
            ));
        }
        if status.playing
            && state
                .voice_next_tick
                .is_none_or(|due| elapsed + 0.01 >= due)
        {
            state.voice_next_tick = Some(elapsed + 0.1);
            tx.wake_after(Duration::from_millis(100));
        } else if !status.playing {
            state.voice_next_tick = None;
        }
        if voice.last_status.as_ref() != Some(&status) {
            *VOICE_STATUS.lock().unwrap_or_else(|e| e.into_inner()) = Some(status.clone());
            voice.last_status = Some(status);
            changed = true;
        }
    }
    if let Some(video) = &state.video {
        let status = video.status();
        if status.playing
            && state
                .video_next_tick
                .is_none_or(|due| elapsed + 0.01 >= due)
        {
            state.video_next_tick = Some(elapsed + 0.1);
            tx.wake_after(Duration::from_millis(100));
        } else if !status.playing {
            state.video_next_tick = None;
        }
        if state.video_status.as_ref() != Some(&status) {
            *VIDEO_STATUS.lock().unwrap_or_else(|e| e.into_inner()) = Some(status.clone());
            state.video_status = Some(status);
            changed = true;
        }
    }
    changed |= stories::poll(state, elapsed, tx);
    changed |= search::poll(state, elapsed, tx);
    if let Some(at) = state.composer_focus_at {
        if let Some(delay) = at.checked_duration_since(std::time::Instant::now()) {
            tx.wake_after(delay);
        } else {
            state.composer_focus_at = None;
            state.focus_composer();
            changed = true;
        }
    }
    if let Some(at) = state.search_focus_at {
        if let Some(delay) = at.checked_duration_since(std::time::Instant::now()) {
            tx.wake_after(delay);
        } else {
            state.search_focus_at = None;
            if state.chat_search.open {
                state.focus_chat_search();
                changed = true;
            } else if state.search.open {
                state.search.edit.focused = true;
                changed = true;
            }
        }
    }
    let second = crate::model::now();
    if state.typing_text != state.draft.text {
        state.typing_text = state.draft.text.clone();
        if state.preferences.value.privacy.send_typing
            && second - state.typing_sent_at >= 4
            && state.authenticated
            && state.editing.is_none()
        {
            if let Some(chat) = state.selected.clone() {
                let _ = state.tx.send(Command::Preferences(
                    "typing",
                    json!({"chat":chat,"active":!state.draft.text.is_empty()}),
                ));
                state.typing_sent_at = second;
            }
        }
        tx.wake_after(Duration::from_secs(5));
    } else if state.typing_sent_at > 0 && second - state.typing_sent_at >= 5 {
        if let Some(chat) = state.selected.clone() {
            let _ = state.tx.send(Command::Preferences(
                "typing",
                json!({"chat":chat,"active":false}),
            ));
        }
        state.typing_sent_at = 0;
    }
    if times::day(second) != times::day(state.second) {
        changed = true;
    }
    if state.attachments.as_ref().is_some_and(|p| {
        !p.sending
            && (!state.authenticated
                || (state.selected.as_ref() != Some(&p.chat)
                    && !state
                        .snapshot
                        .current_chat
                        .as_ref()
                        .is_some_and(|c| c.matches_jid(&p.chat))))
    }) {
        state.cancel_attachments();
        changed = true;
    }
    changed |= state.poll_recording(elapsed, tx);
    let connection = &state.snapshot.connection;
    let ticking = !state.authenticated
        && state.phone_mode
        && [connection.code_expires_at, connection.pair_retry_at]
            .into_iter()
            .flatten()
            .any(|expires| expires > second);
    if ticking {
        // Keep one countdown wake pending. Scheduling on every animation frame
        // would turn those wakes into a permanent stream of redraws.
        if state.next_tick.is_none_or(|due| elapsed + 0.01 >= due) {
            state.next_tick = Some(elapsed + 1.0);
            tx.wake_after(Duration::from_secs(1));
        }
        if second != state.second {
            state.second = second;
            changed = true;
        }
    } else {
        if state.next_tick.take().is_some() && state.second != second {
            changed = true;
        }
        state.second = second;
    }
    if changed {
        state.sync_floating();
    }
    changed
}

fn receive_snapshot(state: &mut State, mut snapshot: Snapshot, tx: &mut Tx) -> bool {
    // An in-flight pre-logout snapshot must not reopen cached chats while the
    // unlink request runs in the background. Fresh pairing snapshots are safe.
    if state.preferences.logging_out
        && (snapshot.connection.has_session
            || snapshot.connection.phase == crate::model::Phase::Connected)
    {
        return false;
    }
    let mut recovered = state.startup_error.take().is_some();
    // Clear only the refresh error: a successful poll must not dismiss an
    // unrelated send, attachment, or update failure. Do this even when the
    // snapshot itself is unchanged so the recovery schedules a redraw.
    if state.error.as_deref() == Some(SNAPSHOT_LOAD_ERROR) {
        state.error = None;
        recovered = true;
    }
    if let Some(viewing) = state.viewing_sent.as_deref() {
        for chat in &mut snapshot.chats {
            if chat.matches_jid(viewing) {
                chat.unread = 0;
            }
        }
        if let Some(chat) = &mut snapshot.current_chat
            && chat.matches_jid(viewing)
        {
            chat.unread = 0;
        }
    }
    let previous = state.snapshot.live_call.clone();
    let call_changed = previous != snapshot.live_call;
    state.update_call(previous.as_ref(), snapshot.live_call.as_ref());
    state.snapshot.live_call = snapshot.live_call.clone();
    if snapshot.selected_chat != state.selected {
        let same_person = state.selected.as_ref().is_some_and(|old| {
            snapshot
                .current_chat
                .as_ref()
                .is_some_and(|chat| chat.matches_jid(old))
        });
        if !same_person {
            if snapshot.selected_chat.is_none()
                && state
                    .selected
                    .as_ref()
                    .is_some_and(|jid| !snapshot.chats.iter().any(|chat| chat.matches_jid(jid)))
            {
                state.selected = None;
                state.timeline.clear();
                state.history_focus = false;
                state.history_generation = state.history_generation.wrapping_add(1);
                state.stop_voice();
                state.close_viewer();
                state.details = false;
                state.picker.open = false;
                state.chat_search.clear();
                state.chat_search.open = false;
            } else {
                return call_changed || recovered;
            }
        }
        state.selected = snapshot.selected_chat.clone();
        if let Some(chat) = state.selected.as_ref() {
            for message in &mut state.timeline {
                message.chat = chat.clone();
            }
        }
    }
    if state.snapshot != snapshot {
        let was_authenticated = state.authenticated;
        let initial_messages = state.timeline.is_empty() && !snapshot.messages.is_empty();
        if state.timeline.is_empty() {
            state.has_older = snapshot.messages.len() == crate::history::CHAT_PAGE_SIZE;
        }
        if !state.history_focus {
            state.merge_messages(snapshot.messages.clone());
        }
        if state.loading_chat && snapshot.selected_chat == state.selected {
            state.loading_chat = false;
            state.history_notice = None;
        }
        state.authenticated = (snapshot.connection.has_session
            || snapshot.connection.phase == crate::model::Phase::Connected)
            && !snapshot.connection.initial_sync;
        if state.authenticated || snapshot.connection.initial_sync {
            state.qr_payload = None;
            state.qr = None;
        } else if let Some(payload) = &snapshot.connection.qr
            && state.qr_payload.as_ref() != Some(payload)
            && let Some(image) = state
                .image_cache
                .as_ref()
                .and_then(|cache| make_qr(payload, cache))
        {
            // Replace atomically once the next code can be drawn.
            // Reconnecting snapshots must not blank the previous QR.
            state.qr_payload = Some(payload.clone());
            state.qr = Some(image);
        }
        if snapshot.revision < state.preferences.settings_revision {
            snapshot.settings = state.snapshot.settings.clone();
        } else if !state.preferences.saving {
            state.preferences.value = snapshot.settings.clone();
        }
        state
            .search
            .results
            .chats
            .retain(|hit| snapshot.chats.iter().any(|chat| chat.matches_jid(&hit.jid)));
        state.search.results.messages.retain(|hit| {
            snapshot
                .chats
                .iter()
                .any(|chat| chat.matches_jid(&hit.chat.jid))
        });
        state.chat_search.results.messages.retain(|hit| {
            snapshot
                .chats
                .iter()
                .any(|chat| chat.matches_jid(&hit.chat.jid))
        });
        let refreshed = state.snapshot.revision != snapshot.revision;
        state.snapshot = snapshot;
        state.update_badge();
        state.open_pending_link();
        if state.section == Section::Status {
            state.request_status_avatars();
        }
        if refreshed && state.picker.open && !state.picker.loading {
            state.load_picker_catalog(false);
        }
        state.request_chat_media();
        if initial_messages {
            // Message rows already lay out instantly. Keep the startup logo's
            // flight when the first history arrives during authentication.
            if was_authenticated {
                tx.instant();
            }
            tx.scroll_to_bottom_instant("messages");
        }
        return true;
    }
    call_changed || recovered
}

fn make_qr(payload: &str, cache: &ImageCache) -> Option<PairingQr> {
    let rendered = qr::render(payload)?;
    let pixels = rendered.pixels;
    let size = [pixels.width(), pixels.height()];
    Some(PairingQr {
        image: cache.rgba(size[0], size[1], pixels.into_raw()),
        size,
        logo: rendered.logo,
    })
}

impl State {
    fn receive_call_result(&mut self, result: Result<Value, String>) {
        self.call.pending = false;
        if let Err(error) = result {
            log::warn!("Call action failed: {error}");
            let message = if locale::has_key(&error) {
                error
            } else {
                "Could not complete the call action. Please try again.".into()
            };
            self.call.error = Some(message.clone());
            if self.snapshot.live_call.is_none() {
                self.error = Some(message);
            }
        }
        self.sync_floating();
    }
    fn receive_call_frame(
        &mut self,
        id: String,
        local: bool,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        pending: Arc<std::sync::atomic::AtomicBool>,
    ) -> bool {
        pending.store(false, std::sync::atomic::Ordering::Release);
        if self
            .snapshot
            .live_call
            .as_ref()
            .is_some_and(|c| c.id == id && !c.ended())
        {
            let pixels = if local {
                &self.call.local
            } else {
                &self.call.remote
            };
            pixels.set(width, height, rgba);
            true
        } else {
            false
        }
    }
    fn merge_messages(&mut self, incoming: Vec<crate::model::Message>) {
        let mut incoming = incoming;
        for message in &incoming {
            if crate::outbox::confirmed(&message.status) {
                self.pending_sends.remove(&message.id);
            }
        }
        incoming.extend(
            self.pending_sends
                .values()
                .filter(|(m, _)| self.selected.as_deref() == Some(&m.chat))
                .map(|(m, _)| m.clone()),
        );
        for mut message in incoming {
            if let Some(existing) = self.timeline.iter_mut().find(|m| {
                m.chat == message.chat
                    && m.id == message.id
                    && (message.id.starts_with(crate::calls::CALL_LOG_PREFIX)
                        || if m.chat.ends_with("@g.us") {
                            m.sender == message.sender
                        } else {
                            m.from_me == message.from_me
                        })
            }) {
                // A stale snapshot must not turn an acknowledged send back
                // into a clock, or hide a retry while its RPC is in flight.
                if existing.from_me
                    && crate::outbox::confirmed(&existing.status)
                    && crate::storage::status_rank(&existing.status)
                        > crate::storage::status_rank(&message.status)
                {
                    message.status = existing.status.clone();
                }
                *existing = message;
            } else {
                self.timeline.push(message);
            }
        }
        self.timeline
            .sort_by(|a, b| (a.timestamp, &a.id, &a.sender).cmp(&(b.timestamp, &b.id, &b.sender)));
    }

    /// Chats with unread messages, leaving out muted and archived ones.
    fn unread_total(&self) -> u32 {
        self.snapshot
            .chats
            .iter()
            .filter(|chat| !chat.muted && !chat.archived && chat.unread > 0)
            .count() as u32
    }

    /// Mirrors the unread count onto the taskbar icon and the tray tooltip.
    fn update_badge(&mut self) {
        let count = if self.authenticated {
            self.unread_total()
        } else {
            0
        };
        if self.badge == Some(count) {
            return;
        }
        let title = if self.config.demo {
            "Brazap — Demo"
        } else {
            "Brazap"
        };
        let description = match count {
            0 => String::new(),
            1 => locale::tr("1 unread chat"),
            _ => locale::format("{count} unread chats", &[("count", count.to_string())]),
        };
        if let Some(tray) = &self.background.tray {
            tray.set_tooltip(if description.is_empty() {
                title.to_owned()
            } else {
                format!("{title} — {description}")
            });
        }
        #[cfg(target_os = "windows")]
        {
            let applied = self.window.lock().is_ok_and(|window| {
                window.as_ref().is_some_and(|window| {
                    crate::windows_integration::set_unread_badge(window, count, &description)
                })
            });
            // A refused overlay is retried on the next snapshot or focus change.
            self.badge = applied.then_some(count);
            return;
        }
        #[cfg(not(target_os = "windows"))]
        {
            self.badge = Some(count);
        }
    }

    /// Opens a chat with the number of a contact card shared in a message.
    fn open_shared_contact(&mut self, number: String) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let _ = self.tx.send(Command::Open(number));
    }

    /// Opens a WhatsApp link from outside once connected; the number check needs WhatsApp.
    fn open_pending_link(&mut self) {
        let ready = self.config.demo
            || (self.authenticated
                && self.snapshot.connection.phase == crate::model::Phase::Connected);
        if !ready || self.busy {
            return;
        }
        if let Some(link) = self.pending_link.take() {
            self.busy = true;
            self.error = None;
            let _ = self.tx.send(Command::Open(link));
        }
    }

    fn update_viewing(&mut self) {
        let chat = if self.focused
            && self.authenticated
            && self.section == Section::Messages
            && !self.preferences.open
            && !self.details
            && self.contact.is_none()
            && self.viewer.is_none()
            && self.story.is_none()
            && !self.diagnostics.open
            && self.attachments.is_none()
            && self
                .snapshot
                .live_call
                .as_ref()
                .is_none_or(|_| self.call.minimized)
        {
            self.selected.clone()
        } else {
            None
        };
        if chat != self.viewing_sent {
            self.viewing_sent = chat.clone();
            let _ = self.tx.send(Command::Viewing(chat));
        }
    }

    fn is_current_chat(&self, chat: &str) -> bool {
        self.section == Section::Messages
            && !self.preferences.open
            && self.story.is_none()
            && self.selected.as_ref().is_some_and(|selected| {
                selected == chat
                    || self
                        .snapshot
                        .chats
                        .iter()
                        .any(|c| c.matches_jid(chat) && c.matches_jid(selected))
            })
    }

    fn select(&mut self, chat: String) {
        self.select_chat(chat, false);
    }

    fn select_chat(&mut self, chat: String, force: bool) {
        if !force && self.is_current_chat(&chat) {
            return;
        }
        let chat = self
            .snapshot
            .chats
            .iter()
            .find(|c| c.matches_jid(&chat))
            .map(|c| c.jid.clone())
            .unwrap_or(chat);
        if self.attachments.as_ref().is_some_and(|p| p.chat != chat) {
            self.error = Some("Finish or discard the attachments before switching chats.".into());
            return;
        }
        if self
            .recording
            .as_ref()
            .is_some_and(|recording| recording.chat != chat)
        {
            self.error =
                Some("Finish or discard the voice recording before switching chats.".into());
            return;
        }
        if self.forwarding.as_ref().is_some_and(|f| f.sending) {
            return;
        }
        self.preferences.open = false;
        self.picker.open = false;
        self.picker.images.clear();
        self.contact = None;
        self.stop_voice();
        self.reply = None;
        // Keep the draft the edit replaced, not the unsaved edit.
        if let Some((_, draft)) = self.editing.take() {
            self.draft = TextEdit::new(draft);
        }
        self.chat_search.clear();
        self.chat_search.open = false;
        self.reaction_menu = None;
        self.forwarding = None;
        if let Some(old) = &self.selected {
            self.drafts.insert(old.clone(), self.draft.text.clone());
        }
        self.draft = TextEdit::new(self.drafts.get(&chat).cloned().unwrap_or_default());
        self.focus_composer();
        self.typing_text = self.draft.text.clone();
        self.typing_sent_at = 0;
        self.selected = Some(chat.clone());
        for row in &mut self.snapshot.chats {
            if row.matches_jid(&chat) {
                row.unread = 0;
            }
        }
        self.snapshot.current_chat = self.snapshot.chats.iter().find(|c| c.jid == chat).cloned();
        self.snapshot.messages.clear();
        self.timeline.clear();
        self.loading_chat = true;
        self.merge_messages(Vec::new());
        self.has_older = true;
        self.has_newer = false;
        self.loading_newer = false;
        self.history_generation = self.history_generation.wrapping_add(1);
        self.loading_older = false;
        self.details = false;
        self.gallery.clear();
        self.gallery_next = None;
        self.gallery_loading = false;
        self.media_menu = None;
        self.visible_media.clear();
        self.prefetched.clear();
        self.transcript_request.clear();
        self.transcript_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Keep finished results; work in progress is asked for again on return.
        self.transcripts.retain(|_, t| t.finished());
        self.detached = false;
        self.history_focus = false;
        self.highlighted = None;
        self.history_notice = None;
        self.attachments_open = false;
        self.close_viewer();
        self.section = Section::Messages;
        // A chat opened from search, a notification or a link shows on its own screen,
        // or stays on All, which lists it too.
        let all = self.chat_kind == sidebar::ChatKind::All;
        match self.snapshot.chats.iter().find(|c| c.jid == chat) {
            Some(listed) => {
                if !(all && self.chat_kind.includes(listed)) {
                    self.chat_kind = sidebar::ChatKind::of(listed);
                }
                self.show_archived = listed.archived;
            }
            None if all => {}
            None => self.chat_kind = sidebar::ChatKind::of_jid(&chat),
        }
        self.story = None;
        self.stop_voice();
        let _ = self.tx.send(Command::Select(chat, self.history_generation));
        self.update_viewing();
    }
    fn focus_composer(&mut self) {
        self.search_focus_at = None;
        self.search.edit.focused = false;
        self.chat_search.edit.focused = false;
        self.draft.focused = true;
    }
    fn defer_composer_focus(&mut self, tx: &mut Tx) {
        // A pointer press can blur an editor in the same frame. Request focus
        // after that frame has committed, without replacing the draft/undo state.
        self.draft.focused = false;
        self.search.edit.focused = false;
        self.chat_search.edit.focused = false;
        self.composer_focus_at = Some(std::time::Instant::now() + Duration::from_millis(16));
        tx.wake_after(Duration::from_millis(16));
    }
    /// Sends a sticker without holding up anything else, so several can go out in a row.
    fn send_sticker(&mut self, chat: String, hash: String) {
        self.error = None;
        if self.tx.send(Command::SendSticker(chat, hash)).is_err() {
            self.error = Some("The local service stopped".into());
        }
    }
    fn tool(&mut self, name: &'static str, args: Value) {
        self.busy = true;
        self.error = None;
        if self.tx.send(Command::Tool(name, args)).is_err() {
            self.busy = false;
            self.error = Some("The local service stopped".into());
        }
    }
    /// Put the composer into edit mode for one of your messages.
    fn begin_edit(&mut self, message: Message, tx: &mut Tx) {
        if self.recording.is_some() || self.attachments.is_some() {
            return;
        }
        let draft = match self.editing.take() {
            Some((_, draft)) => draft,
            None => self.draft.text.clone(),
        };
        self.reply = None;
        self.attachments_open = false;
        self.draft = TextEdit::new(message.text.clone());
        self.editing = Some((message, draft));
        self.defer_composer_focus(tx);
    }
    fn cancel_edit(&mut self) -> bool {
        let Some((_, draft)) = self.editing.take() else {
            return false;
        };
        self.draft = TextEdit::new(draft);
        self.focus_composer();
        true
    }
    fn save_edit(&mut self) {
        let Some((original, _)) = &self.editing else {
            return;
        };
        let text = self.draft.text.clone();
        if text.trim().is_empty() {
            return;
        }
        let original = original.clone();
        self.cancel_edit();
        if text == original.text {
            return;
        }
        if let Some(reason) = original.edit_blocker(crate::model::now()) {
            self.error = Some(reason.into());
            return;
        }
        // Shown at once; the service's answer replaces it, or restores the original.
        let mut edited = original.clone();
        edited.text = text.clone();
        edited.mentions.clear();
        edited.link_preview = None;
        edited.edited = true;
        edited.edited_at = Some(crate::model::now());
        self.merge_messages(vec![edited]);
        self.error = None;
        if self.tx.send(Command::Edit(original.clone(), text)).is_err() {
            self.merge_messages(vec![original]);
            self.error = Some("The local service stopped".into());
        }
    }
    fn send(&mut self) {
        if self.editing.is_some() {
            self.save_edit();
            return;
        }
        if self.busy
            || self.recording.is_some()
            || self.attachments.is_some()
            || self.draft.text.trim().is_empty()
        {
            return;
        }
        if let Some(chat) = self.selected.clone() {
            let reply = self.reply.take();
            let draft = self.draft.text.clone();
            let (text, mentions, mentions_everyone) = self.outgoing_text(&chat, &draft);
            let message = Message {
                id: crate::outbox::new_id(),
                chat: chat.clone(),
                sender: "self".into(),
                from_me: true,
                timestamp: crate::model::now(),
                text,
                mentions,
                mentions_everyone,
                kind: "text".into(),
                status: "sending".into(),
                quote: reply.as_ref().map(|r| crate::model::Quote {
                    chat: None,
                    id: r.id.clone(),
                    sender: r.sender.clone(),
                    text: if r.text.trim().is_empty() {
                        r.preview()
                    } else {
                        r.text.clone()
                    },
                    kind: match r.kind.as_str() {
                        "image" | "video" | "sticker" | "audio" | "file" => r.kind.clone(),
                        _ => String::new(),
                    },
                    ..Default::default()
                }),
                ..Default::default()
            };
            let target = reply.as_ref().map(Message::cursor);
            self.pending_sends
                .insert(message.id.clone(), (message.clone(), target.clone()));
            self.merge_messages(vec![message.clone()]);
            self.draft.clear();
            self.focus_composer();
            self.drafts.remove(&chat);
            self.detached = false;
            self.error = None;
            self.queue_text(message, target);
        }
    }
    fn queue_text(&mut self, message: Message, reply: Option<MessageCursor>) {
        if self.tx.send(Command::Send(message.clone(), reply)).is_err() {
            if let Some((m, _)) = self.pending_sends.get_mut(&message.id) {
                m.status = "failed".into();
            }
            if let Some(m) = self.timeline.iter_mut().find(|m| m.id == message.id) {
                m.status = "failed".into();
            }
            self.error = Some("The local service stopped".into());
        }
    }
    fn retry_text(&mut self, mut message: Message) {
        if !message.from_me || message.kind != "text" || message.status != "failed" {
            return;
        }
        let reply = self
            .pending_sends
            .get(&message.id)
            .and_then(|(_, r)| r.clone());
        message.status = "sending".into();
        self.pending_sends
            .insert(message.id.clone(), (message.clone(), reply.clone()));
        self.merge_messages(vec![message.clone()]);
        self.error = None;
        self.queue_text(message, reply);
    }
}

fn message_enter(s: &mut State, event: &rune::KeyEvent, tx: &mut Tx) -> bool {
    if event.key != rune::Key::Enter || event.mods.shift {
        return false;
    }
    // The IME owns Return while committing a composition. Repeat must not send
    // another message or activate a different control after the first send.
    if s.draft.preedit().is_some()
        || s.attachments.as_ref().is_some_and(|panel| {
            panel
                .items
                .iter()
                .any(|item| item.caption.preedit().is_some())
        })
    {
        return false;
    }
    if !tx.key_repeat() {
        if s.attachments.is_some() {
            s.send_attachments();
        } else {
            s.send();
        }
    }
    true
}

fn on_key(s: &mut State, event: &rune::KeyEvent, tx: &mut Tx) -> bool {
    if event.key == rune::Key::Escape
        && (s.context_menu.is_some() || s.delete_chat.is_some() || s.media_menu.is_some())
    {
        s.context_menu = None;
        s.delete_chat = None;
        s.media_menu = None;
        return true;
    }
    if s.story.is_some() {
        if s.story.as_ref().is_some_and(|story| story.reply.focused)
            && event.key != rune::Key::Escape
        {
            return false;
        }
        match &event.key {
            rune::Key::Escape => s.close_story(),
            rune::Key::Left => s.step_story(false),
            rune::Key::Right => s.step_story(true),
            rune::Key::Text(key) if key == " " => s.pause_story(),
            _ => return false,
        }
        return true;
    }
    if event.is_command('f')
        && !event.mods.shift
        && s.authenticated
        && s.selected.is_some()
        && s.section == Section::Messages
        && !s.preferences.open
        && !s.details
        && s.viewer.is_none()
        && s.attachments.is_none()
        && s.recording.is_none()
        && s.forwarding.is_none()
        && s.reaction_menu.is_none()
        && !state_call_open(s)
    {
        s.open_chat_search(tx);
        return true;
    }
    if event.is_command('k') && !event.mods.shift && s.authenticated && !state_call_open(s) {
        s.open_search();
        tx.wake_after(Duration::from_millis(16));
        return true;
    }
    if event.is_command('m') && event.mods.shift {
        s.diagnostics.open = !s.diagnostics.open;
        s.diagnostics.next_sample = 0.0;
        return true;
    }
    if event.key == rune::Key::Escape {
        if state_call_open(s) {
            s.minimize_call();
            return true;
        }
        if s.recording.is_some() {
            s.cancel_recording();
            return true;
        }
        if s.picker.open {
            s.picker.open = false;
            s.picker.images.clear();
            return true;
        }
        if s.attachments.is_some() {
            s.cancel_attachments();
            return true;
        }
        if s.preferences.logout || s.preferences.confirm_cache {
            if !s.preferences.logging_out {
                s.preferences.logout = false;
                s.preferences.confirm_cache = false;
            }
            return true;
        }
        if s.contact.take().is_some() {
            return true;
        }
        if s.preferences.open {
            s.preferences.open = false;
            return true;
        }
        if s.diagnostics.open {
            s.diagnostics.open = false;
            return true;
        }
        if s.viewer.is_some() {
            s.close_viewer();
            return true;
        }
        if s.reaction_menu.take().is_some() {
            return true;
        }
        if let Some(flow) = &mut s.forwarding {
            if !flow.sending {
                if flow.picker {
                    flow.picker = false;
                } else {
                    s.forwarding = None;
                }
                return true;
            }
        }
        if s.chat_search.open {
            s.close_chat_search();
            return true;
        }
        if s.cancel_edit() {
            return true;
        }
        if s.reply.take().is_some() {
            return true;
        }
        if s.media_menu.take().is_some() {
            return true;
        }
        if s.details {
            s.details = false;
            s.gallery.clear();
            s.trim_images();
            return true;
        }
        if s.attachments_open {
            s.attachments_open = false;
            return true;
        }
    }
    if s.viewer.is_some()
        && matches!(event.key, rune::Key::Left | rune::Key::Right)
        && s.step_viewer(event.key == rune::Key::Right)
    {
        return true;
    }
    if let Some(video) = &s.video {
        match &event.key {
            rune::Key::Text(key) if key == " " => video.toggle(),
            rune::Key::Text(key) if key.eq_ignore_ascii_case("m") => video.toggle_mute(),
            rune::Key::Left => video.seek(video.status().position - 5.0),
            rune::Key::Right => video.seek(video.status().position + 5.0),
            _ => return false,
        }
        return true;
    }
    false
}

impl State {
    fn jump_to_quote(&mut self, quote: &crate::model::Quote, chat: &str, tx: &mut Tx) {
        if quote.chat.as_deref() == Some(crate::stories::CHAT) {
            self.open_quoted_status(&quote.sender, &quote.id);
            return;
        }
        if self.forwarding.is_some() {
            return;
        }
        self.highlighted = Some(quote.id.clone());
        self.detached = true;
        if let Some(message) = self
            .timeline
            .iter()
            .find(|m| m.chat == chat && m.id == quote.id)
        {
            tx.scroll_center_in(
                "messages",
                format!("message-{}-{}", message.sender, message.id),
            );
            return;
        }
        self.history_focus = true;
        self.history_generation = self.history_generation.wrapping_add(1);
        self.loading_newer = false;
        self.loading_older = true;
        let message = Message {
            id: quote.id.clone(),
            chat: chat.into(),
            sender: quote.sender.clone(),
            ..Default::default()
        };
        let _ = self
            .tx
            .send(Command::Jump(message, self.history_generation));
    }

    fn visible_chats(&self) -> Vec<&Chat> {
        if !self.searching_chats() || self.search.edit.text.trim().is_empty() {
            self.snapshot
                .chats
                .iter()
                .filter(|c| c.archived == self.show_archived && self.chat_kind.includes(c))
                .collect()
        } else {
            search::matching_chats(self)
        }
    }

    fn jump(&mut self, hit: SearchHit) {
        self.select_chat(hit.message.chat.clone(), true);
        self.history_focus = true;
        self.highlighted = Some(hit.message.id.clone());
        self.loading_older = true;
        let _ = self
            .tx
            .send(Command::Jump(hit.message, self.history_generation));
    }

    fn load_older(&mut self) {
        if !self.loading_older
            && let Some(first) = self.timeline.first()
        {
            self.loading_older = true;
            self.history_notice = None;
            let _ = self.tx.send(Command::Older(
                first.chat.clone(),
                first.cursor(),
                self.history_generation,
            ));
        }
    }

    fn load_newer(&mut self) -> bool {
        if self.has_newer
            && !self.loading_newer
            && !self.loading_older
            && let Some(last) = self.timeline.last()
        {
            self.loading_newer = true;
            self.history_notice = None;
            let _ = self.tx.send(Command::Newer(
                last.chat.clone(),
                last.cursor(),
                self.history_generation,
            ));
            return true;
        }
        false
    }

    fn request_asset(&mut self, key: String, request: AssetRequest) {
        let avatar_loaded = key
            .strip_prefix("avatar:")
            .and_then(|k| k.rsplit_once(':'))
            .is_some_and(|(jid, version)| {
                self.avatar_versions
                    .get(jid)
                    .is_some_and(|v| version.parse::<i64>().ok() == Some(*v))
            });
        if avatar_loaded
            || self.images.contains_key(&key)
            || self.animations.contains(&key)
            || self.image_pending.contains(&key)
            || self
                .image_failures
                .get(&key)
                .is_some_and(|(time, _)| crate::model::now() - time < 300)
        {
            return;
        }
        self.image_failures.remove(&key);
        self.image_pending.insert(key.clone());
        if self.tx.send(Command::Asset(key.clone(), request)).is_err() {
            self.image_pending.remove(&key);
        }
    }

    /// Show the profile of someone tagged in a group message.
    fn open_mention(&mut self, mention: &crate::model::Mention) {
        let version = self
            .avatar_versions
            .get(&mention.jid)
            .copied()
            .unwrap_or_default();
        self.request_asset(
            format!("profile:{}:{version}", mention.jid),
            AssetRequest::Profile {
                jid: mention.jid.clone(),
            },
        );
        self.contact = Some(Message {
            sender: mention.jid.clone(),
            sender_name: mention.name.clone(),
            sender_phone: mention.phone.clone(),
            sender_photo_version: version,
            ..Default::default()
        });
    }

    fn request_avatar(&mut self, jid: &str, version: i64) {
        self.request_asset(
            media::avatar_key(jid, version),
            AssetRequest::Avatar {
                jid: jid.to_owned(),
                common_group: None,
            },
        );
    }

    /// Transcribes the voice messages on screen, newest first. Messages in
    /// `retry` forget a failure, or with `redo` replace a finished transcript.
    fn request_transcripts(&mut self, retry: Vec<(String, String)>, redo: bool) {
        let Some(chat) = self.selected.clone() else {
            return;
        };
        if !self.snapshot.settings.transcription.enabled
            || self.snapshot.transcription.status == "unsupported"
        {
            return;
        }
        let items: Vec<(String, String)> = self
            .timeline
            .iter()
            .rev()
            .filter(|m| {
                m.kind == "audio"
                    && self
                        .visible_media
                        .iter()
                        .any(|(sender, id)| *sender == m.sender && *id == m.id)
                    && (retry
                        .iter()
                        .any(|(sender, id)| *sender == m.sender && *id == m.id)
                        || !self.transcript(m).is_some_and(|t| t.finished()))
            })
            .map(|m| (m.sender.clone(), m.id.clone()))
            .collect();
        if items.is_empty() || (retry.is_empty() && items == self.transcript_request) {
            return;
        }
        for (sender, id) in &retry {
            // Shows "Transcribing…" until the first poll answers.
            self.transcripts.insert(
                transcript_key(&chat, sender, id),
                crate::transcription::Transcript {
                    status: "queued".into(),
                    ..Default::default()
                },
            );
        }
        self.transcript_request = items.clone();
        let generation = self
            .transcript_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let _ = self.tx.send(Command::Transcribe(
            self.transcript_generation.clone(),
            generation,
            chat,
            items,
            retry,
            redo,
        ));
    }

    /// The newest transcript of a voice message: polled results first, then
    /// what the message carried when it was loaded.
    fn transcript<'a>(
        &'a self,
        message: &'a Message,
    ) -> Option<&'a crate::transcription::Transcript> {
        self.transcripts
            .get(&transcript_key(&message.chat, &message.sender, &message.id))
            .or(message.transcript.as_ref())
    }

    fn request_chat_media(&mut self) {
        if let Some(chat) = self.snapshot.current_chat.clone() {
            self.request_avatar(&chat.jid, chat.photo_version);
        }
        let mut prefetch = Vec::new();
        for m in &self.timeline {
            // Videos download when played; photos and stickers when a chat opens.
            if matches!(m.kind.as_str(), "image" | "sticker")
                && !m.has_media
                && self
                    .prefetched
                    .insert(format!("{}:{}:{}", m.chat, m.sender, m.id))
            {
                prefetch.push((m.sender.clone(), m.id.clone()));
            }
        }
        if !prefetch.is_empty()
            && let Some(chat) = self.selected.clone()
        {
            for items in prefetch.chunks(100) {
                let _ = self
                    .tx
                    .send(Command::Prefetch(chat.clone(), items.to_vec()));
            }
        }
        let visible: Vec<_> = self
            .timeline
            .iter()
            .filter(|m| {
                self.visible_media
                    .iter()
                    .any(|(sender, id)| *sender == m.sender && *id == m.id)
            })
            .cloned()
            .collect();
        for m in visible {
            if m.link_preview.is_some() {
                let request = AssetRequest::LinkPreview {
                    chat: m.chat.clone(),
                    sender: m.sender.clone(),
                    id: m.id.clone(),
                };
                self.request_asset(request.key(), request);
            }
            if !m.from_me && m.chat.ends_with("@g.us") {
                self.request_asset(
                    media::avatar_key(&m.sender, m.sender_photo_version),
                    AssetRequest::Avatar {
                        jid: m.sender.clone(),
                        common_group: Some(m.chat.clone()),
                    },
                );
            }
            self.request_thumbnail(&m);
            // A reply shows the photo or video it answers, when that is loaded.
            if let Some(quote) = &m.quote
                && !quote.kind.is_empty()
                && let Some(original) = self.timeline.iter().find(|o| o.id == quote.id).cloned()
            {
                self.request_thumbnail(&original);
            }
        }
        self.update_animations();
    }

    fn request_thumbnail(&mut self, m: &Message) {
        if matches!(m.kind.as_str(), "image" | "video" | "sticker") {
            if m.kind != "video"
                && !m.has_media
                && self
                    .prefetched
                    .insert(format!("{}:{}:{}", m.chat, m.sender, m.id))
            {
                let _ = self.tx.send(Command::Prefetch(
                    m.chat.clone(),
                    vec![(m.sender.clone(), m.id.clone())],
                ));
            }
            self.request_asset(
                media::thumbnail_key(m),
                AssetRequest::Thumbnail {
                    chat: m.chat.clone(),
                    sender: m.sender.clone(),
                    id: m.id.clone(),
                },
            );
        }
    }

    fn request_gallery_media(&mut self) {
        let items: Vec<_> = self.gallery.iter().take(9).cloned().collect();
        for m in items {
            self.request_thumbnail(&m);
        }
        self.update_animations();
    }

    fn open_details(&mut self) {
        self.details = true;
        self.error = None;
        self.gallery.clear();
        self.gallery_next = None;
        self.gallery_loading = false;
        self.gallery_kind = "media".into();
        self.load_gallery();
        if let Some(chat) = self.snapshot.current_chat.clone() {
            let key = format!("profile:{}:{}", chat.jid, chat.photo_version);
            self.request_asset(key, AssetRequest::Profile { jid: chat.jid });
        }
    }

    fn load_gallery(&mut self) {
        if !self.gallery_loading
            && let Some(chat) = self.selected.clone()
        {
            self.gallery_loading = true;
            let _ = self.tx.send(Command::Gallery(
                chat,
                self.gallery_kind.clone(),
                self.gallery_next.clone(),
            ));
        }
    }

    fn gallery_tab(&mut self, kind: &str) {
        if self.gallery_kind != kind {
            self.gallery_kind = kind.into();
            self.gallery.clear();
            self.gallery_next = None;
            self.gallery_loading = false;
            self.load_gallery();
        }
    }

    fn trim_images(&mut self) {
        // Bound decoded RGBA/mip data as well as entry count; originals stay in SQLite.
        let mut bytes: u64 = self
            .images
            .values()
            .filter_map(|i| i.size())
            .map(|[w, h]| u64::from(w) * u64::from(h) * 16 / 3)
            .sum();
        while self.image_order.len() > 96 || bytes > 32 * 1024 * 1024 {
            let Some(key) = self.image_order.pop_front() else {
                break;
            };
            if let Some(image) = self.images.remove(&key) {
                if let Some([w, h]) = image.size() {
                    bytes = bytes.saturating_sub(u64::from(w) * u64::from(h) * 16 / 3);
                }
            }
        }
        self.image_sizes
            .retain(|key, _| self.images.contains_key(key) || self.animations.contains(key));
        self.image_failures
            .retain(|_, (at, _)| crate::model::now() - *at < 300);
    }

    fn jump_latest(&mut self, tx: &mut Tx) {
        if self.history_focus
            && let Some(chat) = self.selected.clone()
        {
            self.select_chat(chat, true);
        }
        self.detached = false;
        tx.scroll_to_bottom("messages");
    }

    fn close_viewer(&mut self) {
        // A sticker's sound stops with the preview that played it.
        if let Some(viewer) = &self.viewer
            && self.sticker_sound_on(viewer)
        {
            self.stop_voice();
        }
        self.viewer_generation = self.viewer_generation.wrapping_add(1);
        self.viewer = None;
        self.viewer_zoom = media::Zoom::default();
        self.video = None;
        self.video_status = None;
        *VIDEO_STATUS.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.video_next_tick = None;
        self.video_error = None;
        self.images.retain(|key, _| !key.starts_with("image:"));
        self.image_order.retain(|key| !key.starts_with("image:"));
    }

    fn open_image(&mut self, message: Message) {
        self.stop_voice();
        self.close_viewer();
        self.attachments_open = false;
        self.media_menu = None;
        if message.kind == "video" {
            let _ = self
                .tx
                .send(Command::Video(self.viewer_generation, message.clone()));
            self.viewer = Some(message);
            return;
        }
        let request = AssetRequest::Image {
            chat: message.chat.clone(),
            sender: message.sender.clone(),
            id: message.id.clone(),
        };
        let key = request.key();
        self.image_failures.remove(&key);
        self.request_asset(key, request);
        // Opening a sticker with sound plays it, as phones do.
        let sound = message.sticker_sound.then(|| message.clone());
        self.viewer = Some(message);
        if let Some(sticker) = sound {
            self.toggle_sticker_sound(sticker);
        }
    }
}

fn state_call_open(s: &State) -> bool {
    s.snapshot.live_call.is_some() && !s.call.minimized
}

#[cfg(target_os = "windows")]
pub(crate) fn call_notification_text(value: &str, language: &str) -> String {
    locale::in_language(value, language)
}
