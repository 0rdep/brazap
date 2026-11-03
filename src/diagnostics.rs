use anyhow::Result;
use serde_json::{Value, json};

#[cfg(feature = "desktop")]
pub(crate) static IMAGE_TEXTURES: procwatch::Gauge =
    procwatch::Gauge::new("image_texture_estimate");
#[cfg(all(target_os = "windows", feature = "desktop"))]
pub(crate) static VIDEO_TEXTURES: procwatch::Gauge =
    procwatch::Gauge::new("video_texture_estimate");

#[cfg(all(target_os = "windows", feature = "desktop"))]
fn process() -> Value {
    use windows::Win32::System::{
        ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX, PROCESS_MEMORY_COUNTERS_EX2,
        },
        Threading::{GetCurrentProcess, GetProcessHandleCount},
    };
    let mut memory = PROCESS_MEMORY_COUNTERS_EX2::default();
    memory.cb = std::mem::size_of_val(&memory) as u32;
    let mut handles = 0;
    // The extended structure begins with PROCESS_MEMORY_COUNTERS. Windows uses
    // cb to fill PrivateUsage as well; all pointers refer to local, sized buffers.
    let private_working_set;
    unsafe {
        let process = GetCurrentProcess();
        let extended = GetProcessMemoryInfo(
            process,
            (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX2).cast(),
            memory.cb,
        )
        .is_ok();
        if !extended {
            // Older Windows versions still support the common EX prefix.
            memory.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            if GetProcessMemoryInfo(
                process,
                (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX2).cast(),
                memory.cb,
            )
            .is_err()
            {
                return Value::Null;
            }
        }
        private_working_set = extended.then_some(memory.PrivateWorkingSetSize);
        let _ = GetProcessHandleCount(process, &mut handles);
    }
    json!({"pid":std::process::id(),"private_bytes":memory.PrivateUsage,
        "resident_bytes":memory.WorkingSetSize,"private_working_set_bytes":private_working_set,"peak_resident_bytes":memory.PeakWorkingSetSize,
        "handles":handles,"source":"Windows process counters"})
}

#[cfg(all(target_os = "windows", feature = "desktop"))]
fn gpu() -> Value {
    use windows::{Win32::Graphics::Dxgi::*, core::Interface};
    let mut adapters = Vec::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return Value::Null;
        };
        for index in 0..16 {
            let Ok(adapter) = factory.EnumAdapters1(index) else {
                break;
            };
            let Ok(device) = adapter.cast::<IDXGIAdapter3>() else {
                continue;
            };
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            let len = desc
                .Description
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(desc.Description.len());
            let read = |segment| {
                let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                device
                    .QueryVideoMemoryInfo(0, segment, &mut info)
                    .ok()
                    .map(|_| info.CurrentUsage)
            };
            adapters.push(
                json!({"name":String::from_utf16_lossy(&desc.Description[..len]),
                "local_bytes":read(DXGI_MEMORY_SEGMENT_GROUP_LOCAL),
                "non_local_bytes":read(DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL)}),
            );
        }
    }
    json!(adapters)
}

#[cfg(not(all(target_os = "windows", feature = "desktop")))]
fn gpu() -> Value {
    Value::Null
}

#[cfg(not(all(target_os = "windows", feature = "desktop")))]
fn process() -> Value {
    let usage = procwatch::usage();
    json!({"pid":std::process::id(),"private_bytes":usage.footprint,
        "resident_bytes":usage.resident,"peak_resident_bytes":usage.footprint_peak,
        "handles":usage.fds,"source":"procwatch"})
}

impl crate::service::Service {
    pub async fn diagnostics(&self) -> Result<Value> {
        let heap = procwatch::alloc::heap().map(|heap| {
            json!({
                "live_bytes":heap.live,"peak_bytes":heap.peak,
                "largest_allocation_bytes":heap.largest,"big_blocks":heap.big_blocks,
                "allocations":heap.allocations,"thread_net_bytes":heap.tags
            })
        });
        let process = process();
        let outside_rust = process["private_bytes"]
            .as_i64()
            .zip(heap.as_ref().and_then(|h| h["live_bytes"].as_i64()))
            .map(|(private, rust)| (private - rust).max(0));
        // SQLite's native allocator is outside the Rust global allocator.
        let sqlite_bytes = unsafe { libsqlite3_sys::sqlite3_memory_used() };
        let sdk = if let Ok(client) = self.connected().await {
            let report = client.memory_report().await;
            Some(
                json!({"estimated_retained_bytes":report.total_estimated_bytes(),"collections":report.to_string()}),
            )
        } else {
            None
        };
        #[cfg(feature = "desktop")]
        let ui = crate::ui::memory_report();
        #[cfg(not(feature = "desktop"))]
        let ui = Value::Null;
        Ok(
            json!({"process":process,"gpu":gpu(),"rust_heap":heap,"outside_rust_bytes":outside_rust,
            "background_history":self.history_prefetch.report(),"sqlite_bytes":sqlite_bytes,"gauges":procwatch::gauges(),"whatsapp":sdk,"ui":ui}),
        )
    }
}
