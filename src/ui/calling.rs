mod floating;
use super::{Command, State, locale, media, view::*};
use crate::calling::LiveCall;
pub(super) use floating::Floating;
use rune::{Color, El, Icon, Pixels, Tx, col, el, icon, row};
use rune_kit::glyphs;
use serde_json::{Value, json};

pub(super) const VIDEO: Icon = Icon {
    name: "start-video-call",
    view: 24.0,
    body: r#"<rect x="3" y="5" width="12" height="14" rx="3" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="m15 10 6-4v12l-6-4" fill="none" stroke="currentColor" stroke-width="1.7"/>"#,
};
const MIC: Icon = Icon {
    name: "call-microphone",
    view: 24.0,
    body: r#"<rect x="9" y="3" width="6" height="12" rx="3" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="M6 10v2a6 6 0 0 0 12 0v-2M12 18v3m-4 0h8" fill="none" stroke="currentColor" stroke-width="1.7"/>"#,
};
const HANGUP: Icon = Icon {
    name: "hang-up-call",
    view: 24.0,
    body: r#"<path d="M3 15v-4c4-5 14-5 18 0v4l-5-1v-3a11 11 0 0 0-8 0v3l-5 1Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/>"#,
};
const MINIMIZE: Icon = Icon {
    name: "minimize-call",
    view: 24.0,
    body: r#"<path d="M5 12h14" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>"#,
};
const RED: Color = Color {
    r: 0.76,
    g: 0.075,
    b: 0.12,
    a: 1.0,
};

#[derive(Default)]
pub(super) struct Panel {
    pub minimized: bool,
    pub pending: bool,
    pub error: Option<String>,
    pub local: Pixels,
    pub remote: Pixels,
    pub next_tick: f64,
}

impl State {
    pub(super) fn start_call(&mut self, video: bool) {
        if self.snapshot.live_call.as_ref().is_some_and(|c| !c.ended()) {
            self.call.minimized = false;
            return;
        }
        if self.recording.is_some() {
            self.error =
                Some("Finish or discard your voice recording before starting a call.".into());
            return;
        }
        let Some(chat) = self.selected.clone() else {
            return;
        };
        self.stop_voice();
        self.close_viewer();
        self.call.minimized = false;
        self.call_action("start_call", json!({"chat":chat,"video":video}));
    }
    pub(super) fn call_action(&mut self, action: &'static str, args: Value) {
        if self.call.pending && !matches!(action, "end_call" | "dismiss_call") {
            return;
        }
        if action == "answer_call" && self.recording.is_some() {
            self.error =
                Some("Finish or discard your voice recording before answering a call.".into());
            return;
        }
        if action == "answer_call" {
            self.stop_voice();
            self.close_viewer();
        }
        self.call.pending = true;
        self.call.error = None;
        let _ = self.tx.send(Command::Call(action, args));
        self.sync_floating();
    }
    pub(super) fn minimize_call(&mut self) {
        self.call.minimized = true;
        if let Some(id) = self
            .snapshot
            .live_call
            .as_ref()
            .filter(|c| c.ended())
            .map(|c| c.id.clone())
        {
            self.call_action("dismiss_call", json!({"id":id}));
        }
        self.sync_floating();
    }
    pub(super) fn update_call(&mut self, before: Option<&LiveCall>, after: Option<&LiveCall>) {
        if before.map(|c| &c.id) != after.map(|c| &c.id) {
            self.call = Panel::default();
        }
        if let Some(call) = after {
            if !call.camera {
                self.call.local = Pixels::new();
            }
            if !call.peer_camera {
                self.call.remote = Pixels::new();
            }
            if before.is_none_or(|old| old.id != call.id) {
                self.request_avatar(&call.chat, call.photo_version);
            }
            if call.ended() {
                self.call.local = Pixels::new();
                self.call.remote = Pixels::new();
            }
        }
    }
}

fn status(call: &LiveCall) -> String {
    if let Some(error) = &call.error {
        return locale::tr(if locale::has_key(error) {
            error
        } else {
            "Could not connect the call. Please try again."
        });
    }
    if let Some(started) = call.connected_at {
        let seconds = call
            .ended_at
            .unwrap_or_else(crate::model::now)
            .saturating_sub(started);
        let duration = format!("{}:{:02}", seconds / 60, seconds % 60);
        return if call.ended() {
            locale::format("Call ended · {duration}", &[("duration", duration)])
        } else {
            duration
        };
    }
    locale::tr(match call.phase.as_str() {
        "ringing" => {
            if call.video {
                "Incoming video call"
            } else {
                "Incoming voice call"
            }
        }
        "preparing" => "Preparing call…",
        "calling" => "Calling…",
        "connecting" => "Connecting call…",
        "ended" => "Call ended",
        "failed" => "Call failed",
        _ => "Connected",
    })
}

fn control(
    glyph: &'static Icon,
    label_text: &str,
    selected: bool,
    danger: bool,
    enabled: bool,
    action: impl Fn(&mut State, &mut Tx) + 'static,
) -> El {
    let color = if danger {
        RED
    } else if selected {
        MINT
    } else {
        Color::WHITE.alpha(0.07)
    };
    let text = label_text.to_string();
    col()
        .gap(9.0)
        .items_center()
        .child(
            el().size(58.0, 58.0)
                .radius(29.0)
                .fill(color)
                .border(1.0, Color::WHITE.alpha(0.08))
                .center()
                .role("button")
                .label(locale::tr(&text))
                .help(locale::tr(&text))
                .hover(Color::WHITE.alpha(0.08))
                .on_press_tx(move |s: &mut State, tx| {
                    if enabled && (danger || !s.call.pending) {
                        action(s, tx);
                    }
                })
                .child(icon(glyph, 24.0).color(if enabled {
                    if selected { Color::BLACK } else { INK }
                } else {
                    MUTED
                })),
        )
        .child(label(locale::tr(label_text), 11.0, INK))
}

pub(super) fn overlay(s: &State) -> Option<El> {
    let call = s.snapshot.live_call.as_ref()?;
    let id = call.id.clone();
    let caption = status(call);
    if s.call.minimized {
        if call.ended() {
            return None;
        }
        return Some(
            row()
                .key("current-call-bar")
                .pin(None, Some(98.0), Some(30.0), None)
                .w(310.0)
                .padding(14.0)
                .gap(12.0)
                .center_y()
                .radius(22.0)
                .fill(PANEL)
                .border(1.0, MINT.alpha(0.3))
                .backdrop_blur(18.0)
                .role("button")
                .label(locale::tr("Show current call"))
                .on_press(|s: &mut State| s.show_floating_call())
                .child(icon(&PHONE, 20.0).color(MINT))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(5.0)
                        .child(title(&call.name, 13.0).truncate())
                        .child(label(caption, 11.0, MUTED)),
                )
                .child(icon_button(&HANGUP, "End call", move |s, _| {
                    s.call_action("end_call", json!({"id":id}))
                })),
        );
    }
    let width = (s.chat_bounds[2] + 150.0).clamp(380.0, 760.0);
    let has_video = !call.ended() && (call.camera || (call.phase == "active" && call.peer_camera));
    let height = if has_video { 660.0 } else { 570.0 };
    let mut body = col()
        .key("call-card")
        .w(width)
        .max_w(width)
        .max_h(height)
        .padding(24.0)
        .gap(18.0)
        .radius(28.0)
        .fill(PANEL)
        .border(1.0, Color::WHITE.alpha(0.10))
        .backdrop_blur(24.0)
        .on_press(|_: &mut State| {});
    body = body.child(
        row()
            .w_full()
            .center_y()
            .child(label(
                locale::tr(if call.video {
                    "Video call"
                } else {
                    "Voice call"
                }),
                12.0,
                MUTED,
            ))
            .child(el().grow(1.0))
            .child(icon_button(
                if call.ended() {
                    &glyphs::CLOSE
                } else {
                    &MINIMIZE
                },
                if call.ended() {
                    "Close call"
                } else {
                    "Minimize call"
                },
                |s, _| s.minimize_call(),
            )),
    );
    if has_video {
        let mut canvas = el()
            .w_full()
            .h(280.0)
            .radius(18.0)
            .clip_rounded(18.0)
            .fill(Color::BLACK.alpha(0.4))
            .center();
        if call.peer_camera && s.call.remote.size().0 > 0 {
            canvas = canvas.pixels_fit(&s.call.remote, rune::SurfaceFit::Fit);
        } else {
            canvas = canvas.child(
                col()
                    .gap(10.0)
                    .items_center()
                    .child(media::avatar(
                        s,
                        &call.chat,
                        &call.name,
                        call.photo_version,
                        78.0,
                    ))
                    .child(label(
                        locale::tr(if call.peer_camera {
                            "Waiting for video…"
                        } else {
                            "Camera is off"
                        }),
                        12.0,
                        MUTED,
                    )),
            );
        }
        if call.camera && s.call.local.size().0 > 0 {
            canvas = canvas.child(
                el().pin(None, None, Some(12.0), Some(12.0))
                    .size(150.0, 100.0)
                    .radius(12.0)
                    .clip_rounded(12.0)
                    .border(1.0, Color::WHITE.alpha(0.18))
                    .pixels_fit(&s.call.local, rune::SurfaceFit::Fill)
                    .child(label(locale::tr("You"), 10.0, INK).pin(
                        Some(8.0),
                        None,
                        None,
                        Some(7.0),
                    )),
            );
        }
        body = body.child(canvas);
    } else {
        body = body.child(el().w_full().h(123.0).center().child(media::avatar(
            s,
            &call.chat,
            &call.name,
            call.photo_version,
            106.0,
        )));
    }
    let mut identity = col()
        .w_full()
        .items_center()
        .gap(7.0)
        .child(title(&call.name, 22.0).truncate());
    if !call.phone.is_empty() {
        identity = identity.child(label(&call.phone, 12.0, MUTED));
    }
    body = body.child(identity.child(label(caption, 13.0, MUTED).wrap()));
    if call.camera_fallback && !call.ended() {
        body = body.child(
            label(
                locale::tr("Camera unavailable · sending black video"),
                12.0,
                MUTED,
            )
            .wrap(),
        );
    }
    if let Some(error) = &s.call.error {
        body = body.child(
            label(
                locale::tr(error),
                12.0,
                Color {
                    r: 0.95,
                    g: 0.50,
                    b: 0.55,
                    a: 1.0,
                },
            )
            .wrap(),
        );
    }
    let mut controls = row().w_full().gap(24.0).center();
    if call.ended() {
        let chat = call.chat.clone();
        let video = call.video;
        controls = controls.child(control(
            &PHONE,
            "Call again",
            true,
            false,
            true,
            move |s, _| {
                s.select(chat.clone());
                s.start_call(video);
            },
        ));
    } else if call.incoming && call.phase == "ringing" {
        let voice_id = id.clone();
        controls = controls
            .child(control(
                &HANGUP,
                "Decline",
                false,
                true,
                true,
                move |s, _| s.call_action("end_call", json!({"id":id})),
            ))
            .child(control(
                &PHONE,
                if call.video {
                    "Answer with audio"
                } else {
                    "Answer"
                },
                true,
                false,
                true,
                move |s, _| s.call_action("answer_call", json!({"id":voice_id,"video":false})),
            ));
        if call.video {
            let id = call.id.clone();
            controls = controls.child(control(
                &VIDEO,
                "Answer with video",
                true,
                false,
                true,
                move |s, _| s.call_action("answer_call", json!({"id":id,"video":true})),
            ));
        }
    } else {
        let mic_id = id.clone();
        let camera_id = id.clone();
        let muted = call.muted;
        let camera = call.camera;
        controls = controls
            .child(control(
                &MIC,
                if muted { "Unmute" } else { "Mute" },
                muted,
                false,
                call.phase == "active",
                move |s, _| s.call_action("mute_call", json!({"id":mic_id,"muted":!muted})),
            ))
            .child(control(
                &VIDEO,
                if call.video_requested {
                    "Accept video"
                } else if camera && call.camera_fallback {
                    "Stop video"
                } else if camera {
                    "Turn camera off"
                } else {
                    "Turn camera on"
                },
                camera,
                false,
                call.phase == "active",
                move |s, _| s.call_action("camera_call", json!({"id":camera_id,"enabled":!camera})),
            ))
            .child(control(
                &HANGUP,
                "End call",
                false,
                true,
                true,
                move |s, _| s.call_action("end_call", json!({"id":id})),
            ));
    }
    body = body.child(controls);
    Some(
        el().key("live-call-modal")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(24.0)
            .center()
            .fill(Color::BLACK.alpha(0.60))
            .backdrop_blur(16.0)
            .role("dialog")
            .label(locale::tr("Current call"))
            .on_press(|s: &mut State| s.minimize_call())
            .child(body),
    )
}
