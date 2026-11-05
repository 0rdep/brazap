//! Rune registers the faces supplied to AppSpec; a named style alone does not
//! discover an installed Latin font. Load only the families offered in Settings.
use rune::Family;
use std::{collections::HashSet, path::PathBuf, sync::OnceLock};

struct Fonts {
    bytes: Vec<&'static [u8]>,
    files: HashSet<PathBuf>,
    system: &'static str,
    segoe: bool,
    arial: bool,
}

fn catalog() -> &'static Fonts {
    static FONTS: OnceLock<Fonts> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut fonts = Fonts {
            bytes: vec![
                include_bytes!("../../assets/fonts/Inter-Regular.ttf"),
                include_bytes!("../../assets/fonts/Inter-Medium.ttf"),
                include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"),
                include_bytes!("../../assets/emoji/BrazapEmoji.ttf"),
            ],
            files: HashSet::new(),
            system: "Inter",
            segoe: false,
            arial: false,
        };
        fonts.load_platform();
        fonts
    })
}

impl Fonts {
    fn load(&mut self, file: PathBuf) -> bool {
        if self.files.contains(&file) {
            return true;
        }
        let Ok(bytes) = std::fs::read(&file) else {
            return false;
        };
        if bytes.is_empty() || bytes.len() > 16 * 1024 * 1024 {
            return false;
        }
        // AppSpec requires process-lifetime font bytes. Each selected file is
        // loaded once, shared by the main and floating windows, never per edit.
        self.bytes.push(Box::leak(bytes.into_boxed_slice()));
        self.files.insert(file);
        true
    }

    #[cfg(target_os = "windows")]
    fn load_platform(&mut self) {
        let root = std::env::var_os("WINDIR")
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
            .join("Fonts");
        self.segoe = self.load(root.join("segoeui.ttf"));
        if self.segoe {
            self.system = "Segoe UI";
            for file in ["seguisb.ttf", "segoeuib.ttf", "segoeuii.ttf"] {
                self.load(root.join(file));
            }
        }
        self.arial = self.load(root.join("arial.ttf"));
        if self.arial {
            for file in ["arialbd.ttf", "ariali.ttf"] {
                self.load(root.join(file));
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn load_platform(&mut self) {
        // Rune already loads the macOS system UI face with its symbol fonts.
        self.system = "System Font";
        let root = PathBuf::from("/System/Library/Fonts/Supplemental");
        self.arial = self.load(root.join("Arial.ttf"));
        if self.arial {
            for file in ["Arial Bold.ttf", "Arial Italic.ttf"] {
                self.load(root.join(file));
            }
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    fn load_platform(&mut self) {
        fn resolve(pattern: &str) -> Option<(String, PathBuf)> {
            let output = std::process::Command::new("fc-match")
                .args(["--format=%{family[0]}\n%{file}", pattern])
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let output = String::from_utf8(output.stdout).ok()?;
            let (family, file) = output.split_once('\n')?;
            Some((family.into(), file.into()))
        }
        if let Some((family, file)) = resolve("sans-serif")
            && self.load(file)
        {
            self.system = Box::leak(family.into_boxed_str());
            for pattern in ["sans-serif:weight=bold", "sans-serif:slant=italic"] {
                if let Some((_, file)) = resolve(pattern) {
                    self.load(file);
                }
            }
        }
        for name in ["Segoe UI", "Arial"] {
            if let Some((family, file)) = resolve(name)
                && family == name
                && self.load(file)
            {
                if name == "Arial" {
                    self.arial = true;
                } else {
                    self.segoe = true;
                }
                for style in ["weight=bold", "slant=italic"] {
                    if let Some((_, file)) = resolve(&format!("{name}:{style}")) {
                        self.load(file);
                    }
                }
            }
        }
    }
}

pub(super) fn registered() -> &'static [&'static [u8]] {
    &catalog().bytes
}

pub(super) fn available(name: &str) -> bool {
    match name {
        "Segoe UI" => catalog().segoe,
        "Arial" => catalog().arial,
        _ => true,
    }
}

pub(super) fn family(name: &str) -> Family {
    match name {
        "Inter" => Family::Named("Inter"),
        "Segoe UI" if available(name) => Family::Named("Segoe UI"),
        "Arial" if available(name) => Family::Named("Arial"),
        "monospace" => Family::SystemMono,
        _ => Family::Named(catalog().system),
    }
}
