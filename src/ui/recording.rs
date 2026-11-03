use std::time::Duration;

use rune::{Align, Color, El, Icon, Side, Transition, Tx, Vec2, col, el, icon, row};

use super::{
    Command, State,
    audio::{PAUSE, PLAY},
    locale,
    view::*,
};
use crate::{
    model::{Message, MessageCursor, Phase, VoiceUpload},
    playback,
    recording::{CaptureEvent, Progress, Recorded, Recorder},
};

const MIC: Icon = Icon {
    name: "record-voice-message",
    view: 24.0,
    body: r#"<rect x="9" y="3" width="6" height="12" rx="3" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="M6 10v2a6 6 0 0 0 12 0v-2M12 18v3m-4 0h8" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/>"#,
};
const DISCARD: Icon = Icon {
    name: "discard-voice-recording",
    view: 24.0,
    body: r#"<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 10v7m4-7v7" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

pub(super) struct Panel {
    pub chat: String,
    generation: u64,
    reply: Option<MessageCursor>,
    capture: Option<Recorder>,
    note: Option<Recorded>,
    progress: Progress,
    next_tick: Option<f64>,
    paused: bool,
    transitioning: bool,
    finishing: bool,
    sending: bool,
    send_when_ready: bool,
    preview: Option<tempfile::TempPath>,
    waveform: Vec<u8>,
    player: Option<playback::Player>,
    last_status: Option<playback::Status>,
    error: Option<String>,
}

impl State {
    fn toggle_recording(&mut self) {
        if self.recording.is_some() {
            self.pause_resume_recording();
            return;
        }
        if self.busy
            || self.attachments.is_some()
            || self.snapshot.connection.phase != Phase::Connected
        {
            return;
        }
        let Some(chat) = self.selected.clone() else {
            return;
        };
        self.close_viewer();
        self.stop_voice();
        match Recorder::start(self.config.demo) {
            Ok(capture) => {
                self.recording_generation = self.recording_generation.wrapping_add(1);
                self.error = None;
                self.attachments_open = false;
                self.picker.open = false;
                self.recording = Some(Panel {
                    chat,
                    generation: self.recording_generation,
                    reply: self.reply.as_ref().map(Message::cursor),
                    capture: Some(capture),
                    note: None,
                    progress: Progress::default(),
                    next_tick: None,
                    paused: false,
                    transitioning: false,
                    finishing: false,
                    sending: false,
                    send_when_ready: false,
                    preview: None,
                    waveform: Vec::new(),
                    player: None,
                    last_status: None,
                    error: None,
                });
            }
            Err(error) => {
                log::warn!("Could not create recorder: {error}");
                self.error = Some(
                    "Could not record audio. Check your microphone and its permissions.".into(),
                );
            }
        }
    }

    fn pause_resume_recording(&mut self) {
        let Some(recording) = &mut self.recording else {
            return;
        };
        if recording.sending || recording.finishing || recording.transitioning {
            return;
        }
        let Some(capture) = &recording.capture else {
            return;
        };
        if recording.paused {
            if recording.progress.seconds >= 600.0 {
                return;
            }
            recording.player = None;
            recording.preview = None;
            recording.last_status = None;
            recording.error = None;
            capture.resume();
        } else {
            capture.pause();
        }
        recording.transitioning = true;
        recording.next_tick = None;
    }

    fn play_recording(&mut self) {
        self.close_viewer();
        self.stop_voice();
        let Some(recording) = &mut self.recording else {
            return;
        };
        if !recording.paused || recording.transitioning || recording.finishing || recording.sending
        {
            return;
        }
        if let Some(player) = &recording.player {
            player.toggle();
        } else if let Some(wake) = self.wake.lock().ok().and_then(|w| w.clone())
            && let Some(file) = recording.preview.take()
        {
            recording.player = Some(playback::Player::new_audio(file, wake));
        }
        recording.next_tick = None;
    }

    pub(super) fn cancel_recording(&mut self) {
        if self.recording.as_ref().is_some_and(|r| r.sending) {
            return;
        }
        self.recording = None;
    }

    fn send_recording(&mut self) {
        let Some(recording) = &mut self.recording else {
            return;
        };
        if self.busy
            || recording.sending
            || recording.finishing
            || recording.transitioning
            || recording.progress.seconds < 0.5
        {
            return;
        }
        recording.player = None;
        recording.last_status = None;
        if let Some(note) = &recording.note {
            let upload = VoiceUpload {
                chat: recording.chat.clone(),
                path: note.path.to_string_lossy().into_owned(),
                duration_seconds: note.duration_seconds,
                waveform: note.waveform.clone(),
                reply_to: recording.reply.clone(),
            };
            if self
                .tx
                .send(Command::SendVoice(recording.generation, upload))
                .is_err()
            {
                recording.error = Some("The local service stopped".into());
                return;
            }
            recording.sending = true;
            recording.error = None;
            self.busy = true;
        } else if let Some(capture) = &recording.capture {
            capture.finish();
            recording.finishing = true;
            recording.send_when_ready = true;
            recording.next_tick = None;
        }
    }

    pub(super) fn poll_recording(&mut self, elapsed: f64, tx: &mut Tx) -> bool {
        let Some(recording) = &mut self.recording else {
            return false;
        };
        if !recording.sending
            && (!self.authenticated || self.selected.as_deref() != Some(&recording.chat))
        {
            self.cancel_recording();
            return true;
        }
        let mut changed = false;
        let mut send = false;
        if let Some(capture) = &recording.capture {
            if (!recording.paused || recording.transitioning || recording.finishing)
                && recording.next_tick.is_none_or(|due| elapsed + 0.01 >= due)
            {
                recording.next_tick = Some(elapsed + 0.08);
                tx.wake_after(Duration::from_millis(80));
                let progress = capture.progress();
                changed |= progress != recording.progress;
                recording.progress = progress;
            }
            if let Some(event) = capture.try_event() {
                // Read again at each transition to include the last captured packet.
                recording.progress = capture.progress();
                match event {
                    CaptureEvent::Paused(preview) => {
                        recording.paused = true;
                        recording.transitioning = false;
                        recording.preview = Some(preview.path);
                        recording.waveform = preview.waveform;
                        recording.next_tick = None;
                    }
                    CaptureEvent::Resumed => {
                        recording.paused = false;
                        recording.transitioning = false;
                        recording.next_tick = None;
                    }
                    CaptureEvent::Finished(Ok(note)) => {
                        recording.note = Some(note);
                        recording.capture = None;
                        recording.finishing = false;
                        recording.transitioning = false;
                        recording.next_tick = None;
                        send = recording.send_when_ready;
                    }
                    CaptureEvent::Finished(Err(error)) => {
                        self.error = Some(error);
                        self.recording = None;
                        return true;
                    }
                }
                changed = true;
            }
        }
        if let Some(player) = &recording.player {
            let status = player.status();
            if (status.playing || status.loading)
                && recording.next_tick.is_none_or(|due| elapsed + 0.01 >= due)
            {
                recording.next_tick = Some(elapsed + 0.08);
                tx.wake_after(Duration::from_millis(80));
            } else if !status.playing && !status.loading {
                recording.next_tick = None;
            }
            if recording.last_status.as_ref() != Some(&status) {
                recording.last_status = Some(status);
                changed = true;
            }
        }
        if send {
            self.send_recording();
        }
        changed
    }

    pub(super) fn receive_recording(
        &mut self,
        generation: u64,
        result: Result<Message, String>,
        tx: &mut Tx,
    ) {
        let Some(recording) = &mut self.recording else {
            return;
        };
        if recording.generation != generation {
            return;
        }
        self.busy = false;
        match result {
            Ok(message) => {
                let reply = recording.reply.as_ref().map(|r| r.id.clone());
                if self.reply.as_ref().map(|m| m.id.clone()) == reply {
                    self.reply = None;
                }
                self.recording = None;
                self.merge_messages(vec![message]);
                if !self.detached {
                    tx.scroll_to_bottom_instant("messages");
                }
            }
            Err(error) => {
                log::warn!("Voice message send failed: {error}");
                recording.sending = false;
                recording.error = Some("Could not send the voice message. Try again.".into());
            }
        }
    }
}

fn round_control(
    glyph: &'static Icon,
    name: &'static str,
    size: f32,
    enabled: bool,
    primary: bool,
    action: fn(&mut State),
) -> El {
    el().size(size, size)
        .radius(size / 2.0)
        .center()
        .fill(if primary {
            INK.alpha(0.11)
        } else {
            Color::TRANSPARENT
        })
        .border(1.0, INK.alpha(if primary { 0.08 } else { 0.0 }))
        .hover(INK.alpha(0.10))
        .opacity(if enabled { 1.0 } else { 0.3 })
        .role("button")
        .label(locale::tr(name))
        .help(locale::tr(name))
        .on_press(move |s: &mut State| {
            if enabled {
                action(s);
            }
        })
        .child(
            icon(glyph, if primary { 23.0 } else { 19.0 }).color(if primary { INK } else { MUTED }),
        )
}

fn panel(s: &State) -> El {
    let recording = s.recording.as_ref().unwrap();
    let waiting = recording.sending || recording.finishing || recording.transitioning;
    let playing = recording.last_status.as_ref().is_some_and(|p| p.playing);
    let loading = recording.last_status.as_ref().is_some_and(|p| p.loading);
    let status = if recording.sending {
        "Sending voice message…"
    } else if recording.finishing {
        "Preparing voice message…"
    } else if recording.transitioning {
        "One moment…"
    } else if loading {
        "Loading…"
    } else if playing {
        "Audio preview"
    } else if recording.paused {
        "Recording paused"
    } else if !recording.progress.started {
        "Starting microphone…"
    } else {
        "Recording…"
    };
    let position = recording.last_status.as_ref().map_or(0.0, |p| p.position);
    let duration = recording.progress.seconds;
    let clock = if playing { position } else { duration } as u64;
    let mut wave = row().gap(2.0).center_y();
    for index in 0..32 {
        let level = if recording.paused {
            recording.waveform.get(index * 2).copied().unwrap_or(0)
        } else {
            recording
                .progress
                .levels
                .get(index + 16)
                .copied()
                .unwrap_or(0)
        };
        let active = !recording.paused || (index as f64 / 32.0) < position / duration.max(0.01);
        wave = wave.child(
            el().key(format!("recording-wave-{index}"))
                .w(2.5)
                .h(3.0 + 45.0 * f32::from(level) / 100.0)
                .radius(1.25)
                .fill(if active { MINT } else { MUTED.alpha(0.5) })
                .token_motion(rune::tok::motion::SNAP),
        );
    }
    let waveform = el()
        .w_full()
        .h(80.0)
        .center()
        .radius(16.0)
        .fill(Color::BLACK.alpha(0.16))
        .role("slider")
        .label(locale::tr("Voice message position"))
        .help(locale::tr("Click or drag to seek"))
        .on_drag(|s: &mut State, d, _| {
            if d.phase == rune::Phase::Up
                && !d.cancelled()
                && let Some(recording) = &s.recording
                && let Some(player) = &recording.player
            {
                player
                    .seek((d.x / d.w.max(1.0)).clamp(0.0, 1.0) as f64 * recording.progress.seconds);
            }
        })
        .child(wave);
    let can_resume =
        !waiting && recording.capture.is_some() && (!recording.paused || duration < 600.0);
    let can_preview = recording.paused
        && !waiting
        && duration >= 0.5
        && (recording.preview.is_some() || recording.player.is_some());
    let can_send = !waiting && duration >= 0.5 && !s.busy;
    let mut body = col()
        .w(196.0)
        .padding(16.0)
        .gap(14.0)
        .child(
            row()
                .w_full()
                .center()
                .gap(7.0)
                .child(el().size(5.0, 5.0).radius(2.5).fill(if recording.paused {
                    MUTED
                } else {
                    MINT
                }))
                .child(title(locale::tr("Voice message"), 12.0)),
        )
        .child(
            col()
                .w_full()
                .gap(5.0)
                .items_center()
                .child(
                    title(format!("{:02}:{:02}", clock / 60, clock % 60), 30.0)
                        .font(locale::style(30.0, 560.0).features("'tnum' 1")),
                )
                .child(label(locale::tr(status), 10.0, MUTED)),
        )
        .child(waveform)
        .child(
            row()
                .w_full()
                .center()
                .gap(10.0)
                .child(round_control(
                    &DISCARD,
                    "Discard recording",
                    34.0,
                    !recording.sending,
                    false,
                    State::cancel_recording,
                ))
                .child(round_control(
                    if recording.paused { &MIC } else { &PAUSE },
                    if recording.paused {
                        "Resume recording"
                    } else {
                        "Pause recording"
                    },
                    46.0,
                    can_resume,
                    true,
                    State::pause_resume_recording,
                ))
                .child(round_control(
                    if playing { &PAUSE } else { &PLAY },
                    if playing {
                        "Pause audio preview"
                    } else {
                        "Play audio preview"
                    },
                    34.0,
                    can_preview,
                    false,
                    State::play_recording,
                )),
        )
        .child(
            row()
                .w_full()
                .h(38.0)
                .center()
                .gap(7.0)
                .radius(19.0)
                .fill(MINT.alpha(if can_send { 1.0 } else { 0.25 }))
                .role("button")
                .label(locale::tr("Send voice message"))
                .help(locale::tr("Send voice message"))
                .on_press(move |s: &mut State| {
                    if can_send {
                        s.send_recording();
                    }
                })
                .child(icon(&SEND, 16.0).color(Color::BLACK))
                .child(label(locale::tr("Send"), 12.0, Color::BLACK)),
        );
    if let Some(error) = recording.error.as_ref().or_else(|| {
        recording
            .last_status
            .as_ref()
            .and_then(|p| p.error.as_ref())
    }) {
        let text = if recording.error.is_some() {
            error.as_str()
        } else {
            "Could not preview the recording."
        };
        body = body.child(label(locale::tr(text), 11.0, MUTED).wrap());
    }
    body.key("voice-recording-panel")
        .role("group")
        .label(locale::tr("Voice message"))
        .backdrop_blur(18.0)
        .fill(Color {
            r: 0.009,
            g: 0.018,
            b: 0.023,
            a: 0.94,
        })
        .border(1.0, INK.alpha(0.12))
        .radius(27.0)
        .continuous_corners()
        .shadow(14.0, 40.0, Color::BLACK.alpha(0.40))
        .on_press(|_: &mut State| {})
        .anchored_fit("voice-recording-trigger", Side::Top, Align::End, 12.0)
        .transition(Transition::scale_anchor(0.94, Vec2::new(1.0, 1.0)))
        .token_motion(kit().motion.overlay)
}

pub(super) fn button(s: &State, layer: &mut Vec<El>) -> El {
    let active = s.recording.as_ref();
    let waiting = active.is_some_and(|r| r.sending || r.finishing || r.transitioning);
    let enabled = s.snapshot.connection.phase == Phase::Connected
        && (!s.busy || active.is_some())
        && !waiting;
    let title = if let Some(recording) = active {
        if recording.paused {
            "Resume recording"
        } else {
            "Pause recording"
        }
    } else {
        "Record voice message"
    };
    if active.is_some() {
        layer.push(panel(s));
    }
    el().key("voice-recording-trigger")
        .size(42.0, 42.0)
        .radius(21.0)
        .center()
        .fill(if active.is_some() {
            MINT.alpha(0.20)
        } else {
            INK.alpha(0.04)
        })
        .border(1.0, MINT.alpha(if active.is_some() { 0.25 } else { 0.0 }))
        .hover(MINT.alpha(0.15))
        .opacity(if enabled { 1.0 } else { 0.4 })
        .role("button")
        .label(locale::tr(title))
        .help(locale::tr(title))
        .on_press(move |s: &mut State| {
            if enabled {
                s.toggle_recording();
            }
        })
        .child(icon(&MIC, 21.0).color(MINT))
}
