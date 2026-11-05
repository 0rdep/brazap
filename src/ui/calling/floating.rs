//! A retained native call window. Media and controls stay owned by the main
//! window's service; Pixels are shared instead of decoding a second video feed.
use super::{HANGUP, MIC, VIDEO, status};
use crate::ui::{State, locale, view::*};
use crate::{calling::LiveCall, settings::AppearanceSettings};
use rune::{
    App, AppHandle, AppSpec, Color, Cx, El, Icon, Image, Pixels, Tx, WindowHandle, WindowOptions,
    WindowSpec, col, el, icon, row,
};
use serde_json::{Value, json};
use std::time::Duration;

const RESTORE: Icon = Icon {
    name: "restore-call",
    view: 24.0,
    body: r#"<path d="M9 4H4v5m0-5 7 7m4 9h5v-5m0 5-7-7" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

#[derive(Clone, PartialEq)]
struct Published {
    call: LiveCall,
    appearance: AppearanceSettings,
    pending: bool,
    error: Option<String>,
    avatar: bool,
}
pub(in crate::ui) struct Floating {
    app: AppHandle<WindowState>,
    window: WindowHandle,
    shown: bool,
    placed: bool,
    video: bool,
    last: Option<Published>,
}
struct WindowState {
    parent: AppHandle<State>,
    call: Option<LiveCall>,
    appearance: AppearanceSettings,
    pending: bool,
    error: Option<String>,
    avatar: Option<Image>,
    local: Pixels,
    remote: Pixels,
    second: i64,
}
fn empty() -> WindowState {
    unreachable!("WindowSpec supplies the floating state")
}
impl Floating {
    pub(in crate::ui) fn attach(app: &mut App<State>) -> Self {
        let spec = AppSpec {
            fonts: crate::ui::fonts::registered(),
            families: (
                "Inter, 'Brazap Emoji'",
                "ui-monospace, Inter, 'Brazap Emoji'",
            ),
            min_size: Some((320.0, 200.0)),
            poll: Some(poll),
            on_key: Some(|s, event, _| {
                if event.key == rune::Key::Escape {
                    s.restore();
                    true
                } else {
                    false
                }
            }),
            ..AppSpec::new("Brazap · Call", (380.0, 250.0), empty, view)
        };
        let float = App::new(spec).windowless().on_close(|s, _| s.restore());
        let (handle, window) = app.window(
            WindowSpec::from_app("floating-call", float)
                .state(WindowState {
                    parent: app.handle(),
                    call: None,
                    appearance: Default::default(),
                    pending: false,
                    error: None,
                    avatar: None,
                    local: Pixels::new(),
                    remote: Pixels::new(),
                    second: 0,
                })
                .options(WindowOptions {
                    visible: false,
                    transparent: false,
                    borderless: true,
                    always_on_top: true,
                    resizable: false,
                    min_size: Some((320.0, 200.0)),
                    ..Default::default()
                })
                .on_created(|window| {
                    #[cfg(target_os = "windows")]
                    crate::windows_integration::configure_floating_window(&window);
                    let _ = window;
                }),
        );
        Self {
            app: handle,
            window,
            shown: false,
            placed: false,
            video: false,
            last: None,
        }
    }
    pub(in crate::ui) fn focus(&self) {
        self.window.open();
    }
    fn hide(&mut self) {
        if !self.shown {
            return;
        }
        self.shown = false;
        self.last = None;
        self.window.hide();
        self.app.update(|s, _| {
            s.call = None;
            s.avatar = None;
            s.local = Pixels::new();
            s.remote = Pixels::new();
        });
    }
}
impl State {
    pub(in crate::ui) fn sync_floating(&mut self) {
        let Some(floating) = &mut self.floating else {
            return;
        };
        let Some(call) = self
            .snapshot
            .live_call
            .as_ref()
            .filter(|c| !c.ended() && self.call.minimized)
        else {
            floating.hide();
            return;
        };
        let avatar = self
            .avatars
            .get(&call.chat)
            .filter(|image| image.error().is_none())
            .cloned();
        let published = Published {
            call: call.clone(),
            appearance: self.preferences.value.appearance.clone(),
            pending: self.call.pending,
            error: self.call.error.clone(),
            avatar: avatar.is_some(),
        };
        let video = call.video || call.peer_camera || call.video_requested;
        if !floating.shown || video != floating.video {
            floating.video = video;
            floating.window.resize_instant(if video {
                (400.0, 420.0)
            } else {
                (380.0, 250.0)
            });
        }
        if floating.last.as_ref() != Some(&published) {
            floating.last = Some(published.clone());
            let (local, remote) = (self.call.local.clone(), self.call.remote.clone());
            floating.app.update(move |s, tx| {
                s.call = Some(published.call);
                s.appearance = published.appearance;
                s.pending = published.pending;
                s.error = published.error;
                s.avatar = avatar;
                s.local = local;
                s.remote = remote;
                tx.wake_after(Duration::from_secs(1));
            });
        }
        if !floating.shown {
            floating.shown = true;
            if !floating.placed {
                floating.placed = true;
                floating.window.place(
                    rune::DisplayTarget::UnderPointer,
                    rune::OverlayPlacement::Edge {
                        edge: rune::ScreenEdge::Bottom,
                        margin: 28.0,
                        align: rune::Align::End,
                        inset: 28.0,
                    },
                );
            }
            floating.window.open();
        }
    }
    pub(in crate::ui) fn restore_call(&mut self) {
        self.call.minimized = false;
        self.sync_floating();
        if let Ok(window) = self.window.lock()
            && let Some(window) = window.as_ref()
        {
            window.open();
        }
    }
    pub(in crate::ui) fn show_floating_call(&mut self) {
        self.call.minimized = true;
        self.sync_floating();
        if let Some(floating) = &self.floating {
            floating.focus();
        }
    }
}
impl WindowState {
    fn action(&self, name: &'static str, args: Value) {
        self.parent.update(move |s, _| s.call_action(name, args));
    }
    fn restore(&self) {
        self.parent.update(|s, _| s.restore_call());
    }
}
fn poll(s: &mut WindowState, _: f64, tx: &mut Tx) -> bool {
    if s.call.is_none() {
        return false;
    }
    tx.wake_after(Duration::from_secs(1));
    let second = crate::model::now();
    if second == s.second {
        false
    } else {
        s.second = second;
        true
    }
}
fn button(
    glyph: &'static Icon,
    name: &str,
    selected: bool,
    danger: bool,
    enabled: bool,
    action: impl Fn(&mut WindowState, &mut Tx) + 'static,
) -> El {
    let fill = if danger {
        super::RED
    } else if selected {
        MINT
    } else {
        Color::WHITE.alpha(0.06)
    };
    el().size(42.0, 42.0)
        .radius(21.0)
        .center()
        .fill(fill)
        .role("button")
        .label(locale::tr(name))
        .help(locale::tr(name))
        .hover(Color::WHITE.alpha(0.08))
        .on_press_tx(move |s: &mut WindowState, tx| {
            if enabled {
                action(s, tx);
            }
        })
        .child(icon(glyph, 20.0).color(if !enabled {
            MUTED
        } else if selected {
            Color::BLACK
        } else {
            INK
        }))
}
fn view(s: &WindowState, _: &mut Cx<WindowState>) -> El {
    locale::configure(&s.appearance);
    let Some(call) = &s.call else {
        return el().w_full().h_full().fill(PANEL.alpha(1.0));
    };
    let video = call.video || call.peer_camera || call.video_requested;
    let drag = row()
        .grow(1.0)
        .min_w(0.0)
        .h(38.0)
        .gap(10.0)
        .center_y()
        .window_drag()
        .help(locale::tr("Drag to move the call window"))
        .child(icon(if video { &VIDEO } else { &PHONE }, 16.0).color(MINT))
        .child(title(&call.name, 13.0).truncate());
    let header = row()
        .w_full()
        .h(38.0)
        .gap(10.0)
        .center_y()
        .child(drag)
        .child(
            el().size(30.0, 30.0)
                .radius(9.0)
                .center()
                .role("button")
                .label(locale::tr("Return to Brazap"))
                .help(locale::tr("Return to Brazap"))
                .hover(Color::WHITE.alpha(0.08))
                .on_press_tx(|s: &mut WindowState, _| s.restore())
                .child(icon(&RESTORE, 17.0).color(MUTED)),
        );
    let mut body = col()
        .w_full()
        .h_full()
        .padding(16.0)
        .gap(10.0)
        .radius(22.0)
        .clip_rounded(22.0)
        .fill(PANEL)
        .border(1.0, BORDER)
        .child(header);
    if video {
        let mut canvas = el()
            .w_full()
            .grow(1.0)
            .min_h(110.0)
            .radius(14.0)
            .clip_rounded(14.0)
            .fill(Color::BLACK)
            .center();
        if call.peer_camera && s.remote.size().0 > 0 {
            canvas = canvas.pixels_fit(&s.remote, rune::SurfaceFit::Fit);
        } else {
            canvas = canvas.child(label(
                locale::tr(if call.peer_camera {
                    "Waiting for video…"
                } else {
                    "Camera is off"
                }),
                12.0,
                MUTED,
            ));
        }
        if call.camera && s.local.size().0 > 0 {
            canvas = canvas.child(
                el().pin(None, None, Some(8.0), Some(8.0))
                    .size(105.0, 70.0)
                    .radius(9.0)
                    .clip_rounded(9.0)
                    .border(1.0, Color::WHITE.alpha(0.16))
                    .pixels_fit(&s.local, rune::SurfaceFit::Fit),
            );
        }
        body = body.child(canvas);
    } else {
        let mut avatar = el()
            .size(56.0, 56.0)
            .radius(20.0)
            .clip_rounded(20.0)
            .fill(MINT.alpha(0.12))
            .center();
        if let Some(image) = &s.avatar {
            avatar = avatar.pixels_fit(image.pixels(), rune::SurfaceFit::Fill);
        } else {
            avatar = avatar.child(title(
                call.name
                    .chars()
                    .next()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "?".into()),
                23.0,
            ));
        }
        body = body.child(el().w_full().grow(1.0).center().window_drag().child(avatar));
    }
    let caption = if call.camera_fallback {
        locale::format("{duration} · Black video", &[("duration", status(call))])
    } else {
        status(call)
    };
    body = body.child(el().w_full().center().child(label(caption, 11.0, MUTED)));
    if let Some(error) = &s.error {
        body = body.child(label(locale::tr(error), 11.0, INK).wrap());
    }
    let id = call.id.clone();
    let mut controls = row().w_full().gap(16.0).center();
    if call.incoming && call.phase == "ringing" {
        let voice = id.clone();
        let accept = id.clone();
        controls = controls.child(button(
            &PHONE,
            if call.video {
                "Answer with audio"
            } else {
                "Answer"
            },
            true,
            false,
            !s.pending,
            move |s, _| s.action("answer_call", json!({"id":voice,"video":false})),
        ));
        if call.video {
            controls = controls.child(button(
                &VIDEO,
                "Answer with video",
                true,
                false,
                !s.pending,
                move |s, _| s.action("answer_call", json!({"id":accept,"video":true})),
            ));
        }
    } else {
        let mic = id.clone();
        let cam = id.clone();
        let muted = call.muted;
        let camera = call.camera;
        controls = controls
            .child(button(
                &MIC,
                if muted { "Unmute" } else { "Mute" },
                muted,
                false,
                call.phase == "active" && !s.pending,
                move |s, _| s.action("mute_call", json!({"id":mic,"muted":!muted})),
            ))
            .child(button(
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
                call.phase == "active" && !s.pending,
                move |s, _| s.action("camera_call", json!({"id":cam,"enabled":!camera})),
            ));
    }
    controls = controls.child(button(
        &HANGUP,
        if call.phase == "ringing" {
            "Decline"
        } else {
            "End call"
        },
        false,
        true,
        true,
        move |s, _| s.action("end_call", json!({"id":id})),
    ));
    body = body.child(controls);
    el().w_full().h_full().fill(PANEL.alpha(1.0)).child(body)
}
