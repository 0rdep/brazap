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
}

impl rune::compose::Compositor<super::State> for Renderer {
    fn render(&mut self, frame: &mut rune::compose::Frame<'_, super::State>) {
        frame.draw_ui();
        let interval = if frame.rt.state.diagnostics.open {
            1
        } else {
            5
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
