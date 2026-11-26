//! Sample the renderer's actual allocations, rather than inferring them from RAM.
use serde_json::{Value, json};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

static REPORT: Mutex<Value> = Mutex::new(Value::Null);

pub(super) fn report() -> Value {
    REPORT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[derive(Default)]
pub(super) struct Renderer {
    sampled: Option<Instant>,
    device: Option<rune::compose::DeviceKey>,
    devices: u32,
    slow: SlowFrames,
}

/// UI-thread frame work over [`SLOW_FRAME_MS`], logged at most every ten seconds.
#[derive(Default)]
struct SlowFrames {
    since: Option<Instant>,
    count: u32,
    worst_ms: f64,
}

const SLOW_FRAME_MS: f64 = 100.0;

impl Renderer {
    /// Names the GPU each new device draws with, so a report from a slow
    /// machine says whether it ran on an integrated GPU or in software (WARP).
    fn note_device(&mut self, frame: &rune::compose::Frame<'_, super::State>) {
        let key = frame.device_key();
        if self.device == Some(key) {
            return;
        }
        self.device = Some(key);
        self.devices += 1;
        let info = frame.device.adapter_info();
        let software = format!("{:?}", info.device_type) == "Cpu";
        let line = format!(
            "{} ({:?}, {:?}, driver {} {})",
            info.name, info.device_type, info.backend, info.driver, info.driver_info
        );
        if software {
            log::warn!("Drawing in software, without the GPU: {line}");
        } else if self.devices > 1 {
            log::warn!(
                "The GPU device was recreated ({}x): {line}",
                self.devices - 1
            );
        } else {
            log::warn!("Drawing with {line}");
        }
    }

    fn note_slow(&mut self, update_ms: f64) {
        if update_ms < SLOW_FRAME_MS {
            return;
        }
        let slow = &mut self.slow;
        slow.count += 1;
        slow.worst_ms = slow.worst_ms.max(update_ms);
        if slow
            .since
            .is_some_and(|at| at.elapsed() < Duration::from_secs(10))
        {
            return;
        }
        log::warn!(
            "Slow UI frames: {} over {SLOW_FRAME_MS} ms, worst {:.0} ms",
            slow.count,
            slow.worst_ms
        );
        *slow = SlowFrames {
            since: Some(Instant::now()),
            ..Default::default()
        };
    }
}

impl rune::compose::Compositor<super::State> for Renderer {
    /// Only Rune's own UI draws here, so a timer frame that would build what is
    /// already on screen can skip presenting.
    fn draws_own(&self) -> bool {
        false
    }

    fn render(&mut self, frame: &mut rune::compose::Frame<'_, super::State>) {
        frame.draw_ui();
        self.note_device(frame);
        self.note_slow(frame.update_ms);
        // The allocator report sorts every GPU allocation; sample it often only
        // while someone is looking at it.
        let interval = if frame.rt.state.diagnostics.open {
            1
        } else {
            60
        };
        if self
            .sampled
            .is_some_and(|at| at.elapsed() < Duration::from_secs(interval))
        {
            return;
        }
        self.sampled = Some(Instant::now());
        let layers = frame.ui.layer_stats();
        let allocation = frame.device.generate_allocator_report().map(|mut report| {
            report.allocations.sort_by_key(|allocation| std::cmp::Reverse(allocation.size));
            json!({"allocated_bytes":report.total_allocated_bytes,
                "reserved_bytes":report.total_reserved_bytes,
                "blocks":report.blocks.iter().map(|b|b.size).collect::<Vec<_>>(),
                "largest":report.allocations.iter().take(12).map(|a|json!({"name":a.name,"bytes":a.size})).collect::<Vec<_>>()})
        });
        *REPORT.lock().unwrap_or_else(|e| e.into_inner()) = json!({
            "viewport":frame.size,"layers_bytes":layers.bytes,"layer_targets":layers.targets,
            "extra_passes":layers.extra_passes,"allocator":allocation
        });
    }
}
