#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::{Context, Result};

#[cfg(any(debug_assertions, feature = "memory-profiling"))]
#[global_allocator]
static ALLOCATOR: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

fn main() {
    procwatch::alloc::tag_thread("main/UI");
    if let Err(error) = run() {
        eprintln!("Brazap: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("Brazap {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "Brazap — native WhatsApp client\n\nUsage:\n  brazap             Open the desktop app\n  brazap --headless  Run the shared WhatsApp service without a window\n  brazap --mcp       Serve MCP over stdin/stdout using the running app/service\n  brazap --diagnostics  Report memory/cache diagnostics from the running app\n\n  brazap --demo      Explore fictional data without a WhatsApp account\n  brazap --version   Print the installed version\n\nSet BRAZAP_DATA_DIR to select a different SQLite data directory."
        );
        return Ok(());
    }
    let mut config = brazap::config::Config::load()?;
    if let Some(index) = args.iter().position(|arg| arg == "--data-dir") {
        config.data_dir = args
            .get(index + 1)
            .context("--data-dir needs a path")?
            .into();
        std::fs::create_dir_all(&config.data_dir)?;
    }
    if args.iter().any(|arg| arg == "--demo") {
        if !args.iter().any(|arg| arg == "--data-dir") {
            config.data_dir = config.data_dir.join("demo");
        }
        config.demo = true;
    } else if config.data_dir.join(".brazap-demo").exists() {
        config.demo = true;
    }
    std::fs::create_dir_all(&config.data_dir)?;
    brazap::logging::initialize(&config)?;
    if args.iter().any(|arg| arg == "--mcp") {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(brazap::ipc::Remote::load(&config)?.stdio());
    }
    if args.iter().any(|arg| arg == "--diagnostics") {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let value = runtime.block_on(
            brazap::ipc::Remote::load(&config)?
                .request("brazap/diagnostics", serde_json::json!({})),
        )?;
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--headless-memory") {
        #[cfg(all(target_os = "windows", feature = "desktop"))]
        if !config.demo {
            brazap::windows_integration::initialize(&config);
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(async {
            let instance = brazap::ipc::start_or_connect(config).await?;
            let service = instance
                .owned
                .context("Close the running Brazap instance before measuring the backend alone")?;
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let report = service.diagnostics().await;
            service.shutdown().await;
            println!("{}", serde_json::to_string_pretty(&report?)?);
            Ok(())
        });
    }
    if args.iter().any(|arg| arg == "--headless") {
        #[cfg(all(target_os = "windows", feature = "desktop"))]
        if !config.demo {
            brazap::windows_integration::initialize(&config);
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(async {
            let instance = brazap::ipc::start_or_connect(config).await?;
            eprintln!("Brazap service ready. Connect an MCP client with brazap --mcp.");
            tokio::signal::ctrl_c()
                .await
                .context("Could not listen for shutdown")?;
            if let Some(service) = instance.owned {
                service.shutdown().await;
            }
            Ok(())
        });
    }
    #[cfg(feature = "desktop")]
    {
        let initial_chat = args
            .iter()
            .position(|arg| arg == "--open-chat")
            .map(|index| {
                brazap::notifications::activation_target(
                    args.get(index + 1)
                        .context("--open-chat needs a chat link")?,
                )
            })
            .transpose()?;
        #[cfg(target_os = "windows")]
        if let Some(index) = args.iter().position(|arg| arg == "--notification-preview") {
            let chat = brazap::notifications::chat_from_link(
                args.get(index + 1)
                    .context("--notification-preview needs a chat address")?,
            )?;
            return brazap::windows_integration::preview(&config, &chat);
        }
        let desktop_lease = brazap::ipc::desktop_lease(&config)?;
        if desktop_lease.is_none() {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let activated = runtime.block_on(brazap::ipc::activate_existing(
                &config,
                initial_chat.as_deref(),
            ))?;
            #[cfg(target_os = "windows")]
            if let Some(pid) = activated["pid"].as_u64() {
                brazap::windows_integration::allow_activation(pid as u32);
            }
            return Ok(());
        }
        let mut service_running = false;
        if let Ok(remote) = brazap::ipc::Remote::load(&config) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let activated = runtime.block_on(async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    remote.request("brazap/activate", serde_json::json!({"chat":initial_chat})),
                )
                .await
            });
            service_running = matches!(&activated, Ok(Ok(_)));
            if matches!(activated, Ok(Ok(ref value)) if value["accepted"].as_bool()==Some(true)) {
                #[cfg(target_os = "windows")]
                if let Ok(Ok(value)) = &activated
                    && let Some(pid) = value["pid"].as_u64()
                {
                    brazap::windows_integration::allow_activation(pid as u32);
                }
                return Ok(());
            }
        }
        if !service_running {
            match brazap::updates::install_pending(&config) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(error) => {
                    log::warn!("Could not apply the downloaded update on launch: {error:#}")
                }
            }
        }
        brazap::ui::run(config, initial_chat)
    }
    #[cfg(not(feature = "desktop"))]
    {
        anyhow::bail!(
            "This build has no desktop UI. Use --headless or rebuild with default features."
        )
    }
}
