//! Per-user Windows identity, window icons, and native toast delivery.
use std::{
    path::Path,
    sync::{OnceLock, mpsc},
};

use anyhow::{Context, Result};
use windows::{
    Data::Xml::Dom::XmlDocument,
    UI::Notifications::{ToastNotification, ToastNotificationManager},
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, PROPERTYKEY, WPARAM},
        Graphics::Gdi::{CreateBitmap, DeleteObject},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize, IPersistFile,
                StructuredStorage::{InitPropVariantFromCLSID, PROPVARIANT},
            },
            LibraryLoader::GetModuleHandleW,
            Registry::{
                HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
                RegCreateKeyExW, RegSetValueExW,
            },
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        },
        UI::{
            Shell::{
                IShellLinkW, ITaskbarList3, PropertiesSystem::IPropertyStore,
                SetCurrentProcessExplicitAppUserModelID, ShellLink, TaskbarList,
            },
            WindowsAndMessaging::{
                CreateIconIndirect, HICON, ICONINFO, IMAGE_ICON, LR_SHARED, LoadImageW,
                SendMessageW, WM_SETICON,
            },
        },
    },
    core::{GUID, HSTRING, Interface, PCWSTR},
};

fn app_id(config: &crate::config::Config) -> &'static str {
    if config.demo {
        "Brazap.Demo"
    } else {
        "Brazap.Desktop"
    }
}
fn protocol(config: &crate::config::Config) -> &'static str {
    if config.demo { "brazap-demo" } else { "brazap" }
}
const APP_ID_KEY: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};
const ACTIVATOR_KEY: PROPERTYKEY = PROPERTYKEY {
    fmtid: APP_ID_KEY.fmtid,
    pid: 26,
};
// Protocol-only activation uses a stub identity to retain toasts in Action Center.
const ACTIVATOR: GUID = GUID::from_u128(0xc50284da_78b7_45b0_a68d_c89fc01edc21);
static NOTIFIER: OnceLock<mpsc::SyncSender<(String, String, String, bool)>> = OnceLock::new();

fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

pub fn initialize(config: &crate::config::Config) {
    unsafe {
        if let Err(error) = SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(app_id(config)))
        {
            log::warn!("Could not set Windows application identity: {error}");
        }
    }
    NOTIFIER.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<(String, String, String, bool)>(32);
        let config = config.clone();
        std::thread::spawn(move || {
            procwatch::alloc::tag_thread("Windows notifications");
            if let Err(error) = register(&config) {
                log::warn!("Windows notification registration failed: {error:#}");
                return;
            }
            if let Err(error) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
                log::warn!("Windows notification runtime failed: {error}");
                return;
            }
            for (title, preview, chat, sound) in receiver {
                if let Err(error) = show(&config, &title, &preview, &chat, sound) {
                    log::warn!("Could not show Windows notification: {error:#}");
                }
            }
            unsafe {
                RoUninitialize();
            }
        });
        sender
    });
}

pub fn is_foreground(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe {
        let window = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
        let mut foreground = 0;
        windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
            window,
            Some(&mut foreground),
        );
        foreground == pid
    }
}

pub fn notify(title: String, preview: String, chat: String, sound: bool) {
    if let Some(sender) = NOTIFIER.get() {
        if let Err(error) = sender.try_send((title, preview, chat, sound)) {
            log::warn!("Windows notification queue unavailable: {error}");
        }
    } else {
        log::warn!("Windows notification delivery is not initialized");
    }
}

pub fn set_window_icon(window: &rune::WindowHandle) {
    let _ = window.with_raw_handle(|handle| {
        if let rune::raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw() {
            // Shared resource icons are owned by the module, and live as long as the process.
            unsafe {
                let Ok(module) = GetModuleHandleW(None) else {
                    return;
                };
                for (kind, size) in [(0usize, 32), (1, 64)] {
                    if let Ok(icon) = LoadImageW(
                        Some(HINSTANCE(module.0)),
                        PCWSTR(1usize as *const u16),
                        IMAGE_ICON,
                        size,
                        size,
                        LR_SHARED,
                    ) {
                        SendMessageW(
                            HWND(handle.hwnd.get() as *mut _),
                            WM_SETICON,
                            Some(WPARAM(kind)),
                            Some(LPARAM(icon.0 as isize)),
                        );
                    }
                }
            }
        }
    });
}

thread_local! {
    static TASKBAR: std::cell::RefCell<Option<ITaskbarList3>> = const { std::cell::RefCell::new(None) };
    // Badge icons are tiny and few, so they are kept for the life of the process.
    static BADGES: std::cell::RefCell<std::collections::HashMap<String, HICON>> = Default::default();
}

fn badge_icon(count: u32) -> Option<HICON> {
    let label = crate::badge::label(count);
    if let Some(icon) = BADGES.with_borrow(|icons| icons.get(&label).copied()) {
        return Some(icon);
    }
    let size = crate::badge::SIZE as i32;
    // 32-bit bitmaps hold premultiplied BGRA; the mask is ignored when the colors carry alpha.
    let bgra = crate::badge::render(count)
        .chunks_exact(4)
        .flat_map(|p| {
            let alpha = u16::from(p[3]);
            let scale = |c: u8| ((u16::from(c) * alpha + 127) / 255) as u8;
            [scale(p[2]), scale(p[1]), scale(p[0]), p[3]]
        })
        .collect::<Vec<u8>>();
    unsafe {
        let color = CreateBitmap(size, size, 1, 32, Some(bgra.as_ptr().cast()));
        let mask = CreateBitmap(size, size, 1, 1, None);
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        if let Some(icon) = icon {
            BADGES.with_borrow_mut(|icons| icons.insert(label, icon));
        }
        icon
    }
}

/// Shows `count` unread chats over the taskbar icon, or clears it at zero.
/// Returns whether Windows accepted it; it refuses while the window has no taskbar button.
pub fn set_unread_badge(window: &rune::WindowHandle, count: u32, description: &str) -> bool {
    window
        .with_raw_handle(|handle| {
            let rune::raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw() else {
                return false;
            };
            let hwnd = HWND(handle.hwnd.get() as *mut _);
            unsafe {
                // The window thread is normally an apartment already; a mismatch is harmless.
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let Some(taskbar) = TASKBAR.with_borrow_mut(|cached| {
                    if cached.is_none() {
                        let list: ITaskbarList3 =
                            CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).ok()?;
                        list.HrInit().ok()?;
                        *cached = Some(list);
                    }
                    cached.clone()
                }) else {
                    return false;
                };
                if count == 0 {
                    return taskbar
                        .SetOverlayIcon(hwnd, HICON::default(), PCWSTR::null())
                        .is_ok();
                }
                let Some(icon) = badge_icon(count) else {
                    return false;
                };
                taskbar
                    .SetOverlayIcon(hwnd, icon, &HSTRING::from(description))
                    .is_ok()
            }
        })
        .unwrap_or(false)
}

/// Opaque dark call windows use native rounded corners, without the Windows
/// accent border. The opaque surface also works on GPUs without alpha support.
pub fn configure_floating_window(window: &rune::WindowHandle) {
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        DwmSetWindowAttribute,
    };
    set_window_icon(window);
    let _ = window.with_raw_handle(|handle| {
        if let rune::raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw() {
            let hwnd = HWND(handle.hwnd.get() as *mut _);
            let corners = DWMWCP_ROUND;
            let border = DWMWA_COLOR_NONE;
            // Windows versions without these preferences keep the dark fallback.
            unsafe {
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE,
                    std::ptr::from_ref(&corners).cast(),
                    std::mem::size_of_val(&corners) as u32,
                );
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_BORDER_COLOR,
                    (&border as *const u32).cast(),
                    std::mem::size_of_val(&border) as u32,
                );
            }
        }
    });
}

pub fn allow_activation(pid: u32) {
    // A protocol launch may transfer its foreground permission to the existing UI.
    if pid != 0 {
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid) };
    }
}

fn registry_value(path: &str, name: &str, value: &str) -> Result<()> {
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(path),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let data: Vec<u8> = wide(value).into_iter().flat_map(u16::to_le_bytes).collect();
        let result = RegSetValueExW(key, &HSTRING::from(name), None, REG_SZ, Some(&data)).ok();
        let _ = RegCloseKey(key);
        result?;
    }
    Ok(())
}

fn register(config: &crate::config::Config) -> Result<()> {
    let exe = std::env::current_exe()?;
    let icon = config.data_dir.join("notification-logo.png");
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/notification-logo.png"));
    if std::fs::read(&icon).ok().as_deref() != Some(bytes) {
        std::fs::write(&icon, bytes)?;
    }
    let app_key = format!("Software\\Classes\\AppUserModelId\\{}", app_id(config));
    let scheme = format!("Software\\Classes\\{}", protocol(config));
    registry_value(&app_key, "DisplayName", "Brazap")?;
    registry_value(&app_key, "IconUri", &icon.to_string_lossy())?;
    registry_value(&scheme, "", "URL:Brazap")?;
    registry_value(&scheme, "URL Protocol", "")?;
    registry_value(
        &format!("{scheme}\\DefaultIcon"),
        "",
        &format!("\"{}\",0", exe.display()),
    )?;
    registry_value(
        &format!("{scheme}\\shell\\open\\command"),
        "",
        &format!(
            "\"{}\" --open-chat \"%1\" --data-dir \"{}\"",
            exe.display(),
            config.data_dir.display()
        ),
    )?;
    let programs = std::env::var_os("APPDATA").context("Missing Windows APPDATA directory")?;
    let shortcut = Path::new(&programs).join(if config.demo {
        "Microsoft/Windows/Start Menu/Programs/Brazap Demo.lnk"
    } else {
        "Microsoft/Windows/Start Menu/Programs/Brazap.lnk"
    });
    std::fs::create_dir_all(shortcut.parent().unwrap())?;
    // A dedicated thread owns this STA and all its shell interfaces.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let result = (|| -> Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(PCWSTR(wide(&exe).as_ptr()))?;
            link.SetWorkingDirectory(PCWSTR(wide(exe.parent().unwrap()).as_ptr()))?;
            link.SetIconLocation(PCWSTR(wide(&exe).as_ptr()), 0)?;
            link.SetArguments(&HSTRING::from(format!(
                "--data-dir \"{}\"",
                config.data_dir.display()
            )))?;
            let properties: IPropertyStore = link.cast()?;
            properties.SetValue(&APP_ID_KEY, &PROPVARIANT::from(app_id(config)))?;
            properties.SetValue(&ACTIVATOR_KEY, &InitPropVariantFromCLSID(&ACTIVATOR)?)?;
            properties.Commit()?;
            let file: IPersistFile = link.cast()?;
            file.Save(PCWSTR(wide(&shortcut).as_ptr()), true)?;
            Ok(())
        })();
        CoUninitialize();
        result?;
    }
    Ok(())
}

fn xml(value: &str) -> String {
    value
        .chars()
        .filter(|c| *c >= ' ' || matches!(c, '\n' | '\r' | '\t'))
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn show(
    config: &crate::config::Config,
    title: &str,
    preview: &str,
    chat: &str,
    sound: bool,
) -> Result<()> {
    let image = config.data_dir.join("notification-logo.png");
    let document = XmlDocument::new()?;
    document.LoadXml(&HSTRING::from(format!(
        "<toast activationType=\"protocol\" launch=\"{}://chat/{}\"><visual><binding template=\"ToastGeneric\"><image placement=\"appLogoOverride\" hint-crop=\"circle\" src=\"{}\"/><text>{}</text><text>{}</text></binding></visual>{}</toast>",
        protocol(config), xml(chat), xml(&image.to_string_lossy()), xml(title), xml(preview), if sound { "<audio src=\"ms-winsoundevent:Notification.IM\"/>" } else { "<audio silent=\"true\"/>" }
    )))?;
    let notification = ToastNotification::CreateToastNotification(&document)?;
    notification.SetTag(&HSTRING::from(
        &blake3::hash(chat.as_bytes()).to_hex()[..16],
    ))?;
    notification.SetGroup(&HSTRING::from("messages"))?;
    if preview == "Notifications are ready. Click to open your chat." {
        notification.SetTag(&HSTRING::from("local-preview"))?;
    }
    // Keep one current notification per conversation instead of stacking a burst.
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id(config)))?
        .Show(&notification)?;
    Ok(())
}

/// A local preview exercises Windows delivery without sending a WhatsApp message.
pub fn preview(config: &crate::config::Config, chat: &str) -> Result<()> {
    register(config)?;
    unsafe {
        RoInitialize(RO_INIT_MULTITHREADED)?;
    }
    let result = show(
        config,
        "Brazap",
        "Notifications are ready. Click to open your chat.",
        chat,
        true,
    );
    if result.is_ok() {
        std::thread::sleep(std::time::Duration::from_millis(500));
        println!(
            "Notification setting: {:?}; retained notifications: {}",
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id(config)))?
                .Setting()?,
            ToastNotificationManager::History()?
                .GetHistoryWithId(&HSTRING::from(app_id(config)))?
                .Size()?
        );
    }
    unsafe {
        RoUninitialize();
    }
    result
}
