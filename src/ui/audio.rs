use super::{Command, State, view::*};
use crate::{model::Message, playback};
use rune::{Color, El, Icon, col, el, icon, row};

pub(super) const PLAY: Icon = Icon {
    name: "play-voice",
    view: 24.0,
    body: r#"<path d="m9 5 11 7-11 7Z" fill="currentColor"/>"#,
};
pub(super) const PAUSE: Icon = Icon {
    name: "pause-voice",
    view: 24.0,
    body: r#"<path d="M7 5h4v14H7zm7 0h4v14h-4z" fill="currentColor"/>"#,
};

pub(super) struct Voice {
    pub message: Message,
    pub player: Option<playback::Player>,
    pub error: Option<String>,
    pub last_status: Option<playback::Status>,
}

impl State {
    pub(super) fn stop_voice(&mut self) {
        self.voice_generation = self.voice_generation.wrapping_add(1);
        self.voice = None;
        self.voice_next_tick = None;
        *super::VOICE_STATUS
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }
    pub(super) fn play_voice(&mut self, message: Message) {
        if let Some(voice) = &self.voice
            && voice.message.id == message.id
            && voice.message.sender == message.sender
            && voice.error.is_none()
            && let Some(player) = &voice.player
            && player.status().error.is_none()
        {
            // A manual pause/replay cancels any queued automatic continuation.
            self.voice_generation = self.voice_generation.wrapping_add(1);
            player.toggle();
            return;
        }
        self.close_viewer();
        self.stop_voice();
        let _ = self
            .tx
            .send(Command::Audio(self.voice_generation, message.clone()));
        self.voice = Some(Voice {
            message,
            player: None,
            error: None,
            last_status: None,
        });
    }
    fn cycle_voice_rate(&mut self) {
        self.voice_rate = if self.voice_rate < 1.25 {
            1.5
        } else if self.voice_rate < 1.75 {
            2.0
        } else {
            1.0
        };
        if let Some(voice) = &self.voice
            && let Some(player) = &voice.player
        {
            player.set_rate(self.voice_rate);
        }
    }
}

/// `width` is the most the transcript below the player may take.
pub(super) fn view(s: &State, message: &Message, width: f32) -> El {
    let active = s.voice.as_ref().filter(|v| {
        v.message.id == message.id
            && v.message.sender == message.sender
            && v.message.chat == message.chat
    });
    let status = active.and_then(|v| v.player.as_ref()).map(|p| p.status());
    let playing = status.as_ref().is_some_and(|p| p.playing);
    let loading = active.is_some_and(|v| v.player.is_none() && v.error.is_none())
        || status.as_ref().is_some_and(|p| p.loading);
    let error = active
        .and_then(|v| v.error.as_ref())
        .or_else(|| status.as_ref().and_then(|p| p.error.as_ref()));
    let duration = status
        .as_ref()
        .and_then(|p| p.duration)
        .unwrap_or(message.duration_seconds.unwrap_or(0) as f64);
    let position = status.as_ref().map_or(0.0, |p| p.position);
    let progress = if duration > 0.0 {
        (position / duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let click = message.clone();
    let mut waveform = row()
        .w(192.0)
        .h(28.0)
        .gap(2.0)
        .center_y()
        .role("slider")
        .label(super::locale::tr("Voice message position"))
        .help(super::locale::tr("Click or drag to seek"))
        .on_drag(move |s: &mut State, d, _| {
            if d.phase == rune::Phase::Up
                && !d.cancelled()
                && let Some(voice) = &s.voice
                && voice.message.id == click.id
                && let Some(player) = &voice.player
            {
                if let Some(duration) = player.status().duration {
                    player.seek((d.x / d.w.max(1.0)).clamp(0.0, 1.0) as f64 * duration);
                }
            }
        });
    let max = message.waveform.iter().copied().max().unwrap_or(1).max(1) as f32;
    for index in 0..48 {
        let height = if message.waveform.is_empty() {
            5.0
        } else {
            5.0 + 23.0 * message.waveform[index * message.waveform.len() / 48] as f32 / max
        };
        waveform = waveform.child(el().w(2.0).h(height).radius(1.0).fill(
            if (index as f64 / 48.0) < progress {
                MINT
            } else {
                MUTED.alpha(0.8)
            },
        ));
    }
    let target = message.clone();
    let mut content = col()
        .gap(4.0)
        .child(
            row()
                .gap(10.0)
                .center_y()
                .child(
                    el().size(36.0, 36.0)
                        .radius(18.0)
                        .center()
                        .fill(MINT.alpha(0.17))
                        .role("button")
                        .label(super::locale::tr(
                            &(if playing {
                                "Pause voice message"
                            } else {
                                "Play voice message"
                            }),
                        ))
                        .help(super::locale::tr(
                            &(if playing {
                                "Pause voice message"
                            } else {
                                "Play voice message"
                            }),
                        ))
                        .on_press(move |s: &mut State| {
                            if s.forwarding.is_some() {
                                s.toggle_forward(&target);
                            } else if s.story.is_some() {
                                s.pause_story();
                            } else {
                                s.play_voice(target.clone());
                            }
                        })
                        .child(icon(if playing { &PAUSE } else { &PLAY }, 20.0).color(MINT)),
                )
                .child(waveform)
                .child(
                    el().size(42.0, 24.0)
                        .center()
                        .radius(12.0)
                        .fill(Color::WHITE.alpha(0.06))
                        .role("button")
                        .label(super::locale::tr("Voice playback speed"))
                        .help(super::locale::tr("Playback speed: 1×, 1.5×, 2×"))
                        .on_press(|s: &mut State| s.cycle_voice_rate())
                        .child(label(format!("{}×", s.voice_rate), 10.0, INK)),
                ),
        )
        .child(
            label(
                super::locale::tr(
                    &(if loading {
                        "Loading…".into()
                    } else if active.is_some() {
                        clock(position)
                    } else {
                        clock(duration)
                    }),
                ),
                10.0,
                MUTED,
            )
            .pl(46.0),
        );
    if let Some(error) = error {
        content = content.child(
            label(super::locale::tr(error), 10.0, MUTED)
                .max_w(290.0)
                .wrap(),
        );
    }
    if let Some(transcript) = transcript(s, message, width.max(TRANSCRIPT_WIDTH)) {
        content = content.child(transcript);
    }
    content
}

pub(super) const TRANSCRIPT: Icon = Icon {
    name: "voice-transcript",
    view: 24.0,
    body: r#"<path d="M4 7h11M4 12h16M4 17h9" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
};

pub(super) const RETRANSCRIBE: Icon = Icon {
    name: "voice-retranscribe",
    view: 24.0,
    body: r#"<path d="M20 11a8 8 0 0 0-14.3-4.9L4 8M4 4v4h4M4 13a8 8 0 0 0 14.3 4.9L20 16m0 4v-4h-4" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

/// "Transcribe again" on a finished transcript, when enabled in Settings.
fn retranscribe(s: &State, message: &Message) -> Option<El> {
    if !s.snapshot.settings.transcription.retranscribe {
        return None;
    }
    let target = (message.sender.clone(), message.id.clone());
    Some(
        el().size(22.0, 22.0)
            .center()
            .radius(11.0)
            .fill(Color::WHITE.alpha(0.06))
            .hover(Color::WHITE.alpha(0.1))
            .role("button")
            .label(super::locale::tr("Transcribe again"))
            .help(super::locale::tr(
                "Transcribe this voice message again with the selected model",
            ))
            .on_press(move |s: &mut State| s.request_transcripts(vec![target.clone()], true))
            .child(icon(&RETRANSCRIBE, 12.0).color(MINT)),
    )
}

/// Which model produced a transcript, for its caption's tooltip.
fn produced_by(transcript: &crate::transcription::Transcript) -> Option<String> {
    let model = crate::transcription::SpeechModel::by_name(&transcript.model)?;
    Some(super::locale::format(
        "Transcribed on this computer with {model}",
        &[("model", model.title.to_owned())],
    ))
}

/// Transcripts longer than this show their beginning with "Show more".
const PREVIEW_CHARS: usize = 320;
/// The voice player's own width, and the transcript's where no wider bubble is known.
pub(super) const TRANSCRIPT_WIDTH: f32 = 290.0;

/// The voice message's text below the player: pending, failed or done.
fn transcript(s: &State, message: &Message, width: f32) -> Option<El> {
    let current = s.transcript(message);
    let requested = s
        .transcript_request
        .iter()
        .any(|(sender, id)| *sender == message.sender && *id == message.id);
    let engine = &s.snapshot.transcription;
    let enabled = s.snapshot.settings.transcription.enabled && engine.status != "unsupported";
    let unfinished = current.is_some_and(|t| !t.finished() && t.error.is_none());
    if unfinished && !enabled {
        return None;
    }
    let pending = enabled && (unfinished || (current.is_none() && requested));
    let note = |text: String| {
        row()
            .gap(6.0)
            .center_y()
            .max_w(width)
            .child(icon(&TRANSCRIPT, 13.0).color(MUTED))
            .child(label(text, 11.0, MUTED).wrap())
    };
    if pending {
        let text = if engine.status == "downloading" {
            super::locale::format(
                "Downloading speech model… {percent}%",
                &[("percent", engine.progress.unwrap_or(0).to_string())],
            )
        } else {
            super::locale::tr("Transcribing…")
        };
        return Some(note(text).pl(46.0).motion(rune::tok::motion::SNAPPY));
    }
    let t = current?;
    let body = if t.status != "done" {
        let target = (message.sender.clone(), message.id.clone());
        let error = t
            .error
            .as_deref()
            .unwrap_or("Could not transcribe this voice message");
        row()
            .gap(8.0)
            .center_y()
            .max_w(width)
            .child(note(super::locale::tr(error)).shrink())
            .child(
                el().px(8.0)
                    .h(22.0)
                    .center()
                    .radius(11.0)
                    .fill(Color::WHITE.alpha(0.06))
                    .role("button")
                    .label(super::locale::tr("Try again"))
                    .help(super::locale::tr("Transcribe this voice message again"))
                    .on_press(move |s: &mut State| {
                        s.request_transcripts(vec![target.clone()], false)
                    })
                    .child(label(super::locale::tr("Try again"), 10.0, MINT)),
            )
    } else if t.text.trim().is_empty() {
        let mut line = row()
            .gap(8.0)
            .center_y()
            .max_w(width)
            .child(note(super::locale::tr("No speech detected")).shrink());
        if let Some(help) = produced_by(t) {
            line = line.help(help);
        }
        if let Some(button) = retranscribe(s, message) {
            line = line.child(button);
        }
        line
    } else {
        let key = super::transcript_key(&message.chat, &message.sender, &message.id);
        let open = s.transcripts_open.contains(&key);
        let long = t.text.chars().count() > PREVIEW_CHARS;
        let mut text = if long && !open {
            preview(&t.text)
        } else {
            t.text.clone()
        };
        if t.partial && (open || !long) {
            text.push(' ');
            text.push_str(&super::locale::tr("(only the first 15 minutes)"));
        }
        let mut caption = row()
            .gap(5.0)
            .center_y()
            .child(icon(&TRANSCRIPT, 12.0).color(MINT.alpha(0.8)))
            .child(label(super::locale::tr("Transcript"), 10.0, MUTED));
        if let Some(help) = produced_by(t) {
            caption = caption.help(help);
        }
        let mut header = row().gap(8.0).center_y().child(caption);
        if let Some(button) = retranscribe(s, message) {
            header = header.child(button);
        }
        let mut body = col().gap(4.0).max_w(width).child(header).child(
            label(text, 12.0, INK.alpha(0.86))
                .wrap()
                .selectable(true)
                .label(super::locale::tr("Voice message transcript")),
        );
        if long {
            let caption = if open { "Show less" } else { "Show more" };
            body = body.child(
                label(super::locale::tr(caption), 11.0, MINT)
                    .role("button")
                    .label(super::locale::tr(caption))
                    .on_press(move |s: &mut State| {
                        if !s.transcripts_open.remove(&key) {
                            s.transcripts_open.insert(key.clone());
                        }
                    }),
            );
        }
        body
    };
    Some(col().pt(2.0).child(body).motion(rune::tok::motion::SNAPPY))
}

/// The beginning of a long transcript, cut at a word boundary.
fn preview(text: &str) -> String {
    let cut: String = text.chars().take(PREVIEW_CHARS).collect();
    let cut = match cut.rfind(char::is_whitespace) {
        Some(index) if index > PREVIEW_CHARS / 2 => &cut[..index],
        _ => cut.as_str(),
    };
    format!(
        "{}…",
        cut.trim_end_matches(|c: char| c.is_whitespace() || c == ',' || c == '.')
    )
}

fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
