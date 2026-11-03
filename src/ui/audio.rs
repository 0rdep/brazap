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
    fn play_voice(&mut self, message: Message) {
        if let Some(voice) = &self.voice
            && voice.message.id == message.id
            && voice.message.sender == message.sender
            && voice.error.is_none()
            && let Some(player) = &voice.player
            && player.status().error.is_none()
        {
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

pub(super) fn view(s: &State, message: &Message) -> El {
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
    content
}

fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
