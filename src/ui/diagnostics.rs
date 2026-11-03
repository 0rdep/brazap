use super::{State, view::*};
use rune::{Color, El, ScrollSpec, col, el, row};
use rune_kit::glyphs;
use serde_json::Value;
use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct Panel {
    pub open: bool,
    pub pending: bool,
    pub next_sample: f64,
    report: Value,
    baseline: Value,
    history: VecDeque<u64>,
    error: Option<String>,
}

impl Panel {
    pub fn receive(&mut self, result: Result<Value, String>) {
        self.pending = false;
        match result {
            Ok(report) => {
                if self.baseline.is_null() {
                    self.baseline = report.clone();
                }
                if let Some(bytes) = report["process"]["private_working_set_bytes"]
                    .as_u64()
                    .or_else(|| report["process"]["resident_bytes"].as_u64())
                {
                    self.history.push_back(bytes);
                    while self.history.len() > 60 {
                        self.history.pop_front();
                    }
                }
                self.report = report;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
}

fn bytes(value: Option<i64>) -> String {
    value
        .map(|b| format!("{:.1} MiB", b as f64 / 1_048_576.0))
        .unwrap_or_else(|| super::locale::tr("Unavailable"))
}

fn metric(name: &str, value: Option<i64>, baseline: Option<i64>) -> El {
    let change = value.zip(baseline).map(|(a, b)| {
        super::locale::format(
            "{value} MiB since baseline",
            &[("value", format!("{:+.1}", (a - b) as f64 / 1_048_576.0))],
        )
    });
    let mut item = col()
        .grow(1.0)
        .min_w(0.0)
        .padding(14.0)
        .gap(7.0)
        .radius(12.0)
        .fill(Color::WHITE.alpha(0.035))
        .border(1.0, BORDER)
        .child(label(super::locale::tr(name), 11.0, MUTED))
        .child(title(bytes(value), 22.0));
    if let Some(change) = change {
        item = item.child(label(change, 10.0, MUTED));
    }
    item
}

fn item(name: &str, value: Option<i64>, description: &str) -> El {
    row()
        .gap(16.0)
        .center_y()
        .py(8.0)
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(4.0)
                .child(label(super::locale::tr(name), 12.0, INK))
                .child(label(super::locale::tr(description), 10.0, MUTED).wrap()),
        )
        .child(label(bytes(value), 12.0, MINT))
}

pub(super) fn modal(s: &State) -> Option<El> {
    let panel = &s.diagnostics;
    if !panel.open {
        return None;
    }
    let report = &panel.report;
    let baseline = &panel.baseline;
    let mut body = col().w_full().gap(16.0).padding(22.0);
    if let Some(error) = &panel.error {
        body = body.child(label(super::locale::tr(error), 12.0, INK).wrap());
    }
    body = body
        .child(
            row()
                .gap(12.0)
                .child(metric(
                    "App RAM (Task Manager)",
                    report["process"]["private_working_set_bytes"]
                        .as_i64()
                        .or_else(|| report["process"]["resident_bytes"].as_i64()),
                    baseline["process"]["private_working_set_bytes"]
                        .as_i64()
                        .or_else(|| baseline["process"]["resident_bytes"].as_i64()),
                ))
                .child(metric(
                    "Private commit",
                    report["process"]["private_bytes"].as_i64(),
                    baseline["process"]["private_bytes"].as_i64(),
                )),
        )
        .child(
            row()
                .gap(12.0)
                .child(metric(
                    "Rust allocations",
                    report["rust_heap"]["live_bytes"].as_i64(),
                    baseline["rust_heap"]["live_bytes"].as_i64(),
                ))
                .child(metric(
                    "Outside counted Rust heap",
                    report["outside_rust_bytes"].as_i64(),
                    baseline["outside_rust_bytes"].as_i64(),
                )),
        );
    if !panel.history.is_empty() {
        let peak = panel.history.iter().copied().max().unwrap_or(1).max(1) as f32;
        let mut graph = row().w_full().h(42.0).gap(2.0);
        for bytes in &panel.history {
            graph = graph.child(
                col().grow(1.0).h_full().child(el().grow(1.0)).child(
                    el().w_full()
                        .h((*bytes as f32 / peak * 40.0).max(2.0))
                        .radius(2.0)
                        .fill(MINT.alpha(0.55)),
                ),
            );
        }
        body = body.child(
            col()
                .gap(8.0)
                .child(label(
                    super::locale::tr("App RAM · last 60 samples"),
                    11.0,
                    MUTED,
                ))
                .child(graph),
        );
    }
    let image = report["ui"]["decoded_image_estimate_bytes"].as_i64();
    let video = report["gauges"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item[0] == "video_texture_estimate")
        })
        .and_then(|item| item[1].as_i64())
        .unwrap_or(0);
    body = body.child(
        col()
            .gap(2.0)
            .child(title(super::locale::tr("What we can measure"), 14.0))
            .child(item(
                "Image pixels",
                image,
                "Estimated decoded images and mipmaps; included in Rust heap.",
            ))
            .child(item(
                "Video textures",
                Some(video),
                "Estimated native playback and display textures.",
            ))
            .child(item(
                "SQLite allocator",
                report["sqlite_bytes"].as_i64(),
                "Measured native SQLite allocations across this process.",
            ))
            .child(item(
                "WhatsApp caches",
                report["whatsapp"]["estimated_retained_bytes"].as_i64(),
                "SDK estimate; included in Rust heap, not additional memory.",
            )),
    );
    let renderer = &report["ui"]["renderer"]["allocator"];
    if !renderer.is_null() {
        body = body
            .child(item(
                "Renderer allocations",
                renderer["allocated_bytes"].as_i64(),
                "Live GPU buffers and textures reported by wgpu.",
            ))
            .child(item(
                "Renderer reserved memory",
                renderer["reserved_bytes"].as_i64(),
                "GPU heap blocks, including unused space reserved by the graphics allocator.",
            ));
    }
    if let Some(adapters) = report["gpu"].as_array() {
        for adapter in adapters {
            let total = adapter["local_bytes"]
                .as_u64()
                .zip(adapter["non_local_bytes"].as_u64())
                .map(|(local, non_local)| (local + non_local) as i64);
            body = body.child(item(
                "GPU-reported usage",
                total,
                adapter["name"].as_str().unwrap_or("Graphics adapter"),
            ));
        }
        body = body.child(label(super::locale::tr("GPU values are per-process DXGI measurements. Shared graphics memory can overlap process RAM; do not add these totals together."), 10.0, MUTED).wrap());
    }
    body = body.child(label(super::locale::tr("The remainder includes allocator overhead, native libraries, graphics drivers and resources we cannot attribute yet. It is not a measurement of GPU memory alone."), 11.0, MUTED).wrap());
    if report["rust_heap"].is_null() {
        body = body.child(
            label(
                super::locale::tr(
                    "Rust allocation tracking needs a build with the memory-profiling feature.",
                ),
                11.0,
                MINT,
            )
            .wrap(),
        );
    }
    if let Some(tags) = report["rust_heap"]["thread_net_bytes"].as_array() {
        let mut section = col().gap(4.0).child(title(
            super::locale::tr("Allocation traffic by thread"),
            14.0,
        ));
        for tag in tags.iter().take(6) {
            section = section.child(
                row()
                    .center_y()
                    .child(label(tag[0].as_str().unwrap_or("Unknown"), 11.0, MUTED).grow(1.0))
                    .child(label(bytes(tag[1].as_i64()), 11.0, INK)),
            );
        }
        body = body.child(section).child(label(super::locale::tr("Net allocations minus frees on each thread. Buffers freed on another thread can produce negative values; these are not ownership totals."), 10.0, MUTED).wrap());
    }
    body = body.child(label(
        super::locale::format(
            "{images} image handles · {messages} timeline messages · {handles} process handles",
            &[
                (
                    "images",
                    report["ui"]["image_count"]
                        .as_u64()
                        .unwrap_or(0)
                        .to_string(),
                ),
                (
                    "messages",
                    report["ui"]["timeline_messages"]
                        .as_u64()
                        .unwrap_or(0)
                        .to_string(),
                ),
                (
                    "handles",
                    report["process"]["handles"]
                        .as_u64()
                        .unwrap_or(0)
                        .to_string(),
                ),
            ],
        ),
        10.0,
        MUTED,
    ));
    let header = row()
        .px(22.0)
        .h(60.0)
        .center_y()
        .child(title(super::locale::tr("Memory diagnostics"), 18.0))
        .child(el().grow(1.0))
        .child(icon_button(&glyphs::COPY, "Copy memory report", |s, tx| {
            if let Ok(text) = serde_json::to_string_pretty(&s.diagnostics.report) {
                tx.copy_rich(rune::ClipboardContent::new(text));
            }
        }))
        .child(icon_button(
            &glyphs::CLOSE,
            "Close memory diagnostics",
            |s, _| s.diagnostics.open = false,
        ));
    let footer = row()
        .px(22.0)
        .pb(18.0)
        .gap(12.0)
        .center_y()
        .child(
            label(
                super::locale::tr("Updates every second while open"),
                10.0,
                MUTED,
            )
            .grow(1.0),
        )
        .child(button("Set baseline", |s, _| {
            s.diagnostics.baseline = s.diagnostics.report.clone();
            s.diagnostics.history.clear();
        }));
    Some(
        el().key("memory-diagnostics")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .center()
            .padding(28.0)
            .fill(Color::BLACK.alpha(0.72))
            // Inspecting memory must not allocate a full-window blur pyramid.
            .role("dialog")
            .label(super::locale::tr("Memory diagnostics"))
            .on_press(|s: &mut State| s.diagnostics.open = false)
            .child(super::view::panel(
                col()
                    .w(660.0)
                    .h_full()
                    .max_h(820.0)
                    .on_press(|_: &mut State| {})
                    .child(header)
                    .child(body.grow(1.0).min_h(0.0).scroll(ScrollSpec {
                        stick_bottom: false,
                    }))
                    .child(footer),
            )),
    )
}
