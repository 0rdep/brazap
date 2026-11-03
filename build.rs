use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=assets/logo.png");
    println!("cargo:rerun-if-env-changed=WINDRES");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let logo = image::open("assets/logo.png").expect("Read the Brazap logo");
    // The artwork's landscape canvas includes a wide glow. Fit the square mark,
    // not that canvas, into Windows' tiny icon slots without stretching it.
    let source = logo.crop_imm(318, 64, 894, 894);
    let frames: Vec<_> = [16, 24, 32, 48, 64, 128, 256]
        .into_iter()
        .map(|size| {
            let pixels = source.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
            let mut png = std::io::Cursor::new(Vec::new());
            pixels.write_to(&mut png, image::ImageFormat::Png).unwrap();
            image::codecs::ico::IcoFrame::with_encoded(
                png.into_inner(),
                size,
                size,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap()
        })
        .collect();
    image::codecs::ico::IcoEncoder::new(fs::File::create(out.join("brazap.ico")).unwrap())
        .encode_images(&frames)
        .expect("Encode the Windows icon");
    source
        .resize_exact(128, 128, image::imageops::FilterType::Lanczos3)
        .save(out.join("notification-logo.png"))
        .unwrap();
    let resource = out.join("brazap.rc");
    let icon = out.join("brazap.ico").to_string_lossy().replace('\\', "/");
    fs::write(&resource, format!("1 ICON \"{icon}\"\n")).unwrap();
    let msvc = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let object = out.join(if msvc { "brazap.res" } else { "brazap-icon.o" });
    let status = if msvc {
        Command::new("rc.exe")
            .arg("/nologo")
            .arg(format!("/fo{}", object.display()))
            .arg(&resource)
            .status()
    } else {
        let target = env::var("TARGET").unwrap();
        let default = if target.starts_with("x86_64-") {
            "x86_64-w64-mingw32-windres"
        } else {
            "windres"
        };
        let mut command = Command::new(env::var("WINDRES").unwrap_or(default.into()));
        if let Ok(cc) = env::var("CC_x86_64_pc_windows_gnu") {
            command.arg(format!("--preprocessor={cc}")).args([
                "--preprocessor-arg=-E",
                "--preprocessor-arg=-xc",
                "--preprocessor-arg=-DRC_INVOKED",
            ]);
        }
        command
            .args(["-O", "coff", "-i"])
            .arg(&resource)
            .arg("-o")
            .arg(&object)
            .status()
    }
    .expect("Run the Windows resource compiler (rc.exe or MinGW windres)");
    assert!(status.success(), "Compile the Windows application icon");
    println!("cargo:rustc-link-arg-bin=brazap={}", object.display());
}
