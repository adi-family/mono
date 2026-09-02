//! Compiles the Windows resources — the icon, the version block and the manifest — into
//! `ADI.exe`.
//!
//! The icon matters more than it looks: it is what the Start-menu entry, the taskbar button and
//! Alt-Tab show, and an app that ships as a generic white rectangle reads as something that
//! escaped from a build directory. It is the same artwork as the macOS bundle's, in a different
//! container: `apps/windows/ADI.ico` is generated from `apps/macos/ADI.icns` by
//! `apps/windows/build.sh --regen-icon`, so the two cannot drift.
//!
//! Everything is generated into `OUT_DIR` and compiled with `windres`, which the mingw-w64
//! cross-toolchain already supplies — the same toolchain that builds every other Windows binary
//! here, so the shipped build always has this. When `windres` is missing (an MSVC build on a real
//! Windows runner, where the only thing being asked is "does it compile") the resources are
//! skipped with a warning rather than failing the build: the binary still works, it is just
//! plain, and nothing that ships is built that way.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../apps/windows/ADI.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = manifest.join("../../apps/windows/ADI.ico");
    if !icon.is_file() {
        println!("cargo:warning=apps/windows/ADI.ico is missing — ADI.exe will have no icon");
        return;
    }

    let Some(windres) = find_windres() else {
        println!("cargo:warning=windres not found — ADI.exe will have no icon or version block");
        return;
    };

    // Copied rather than referenced in place: windres resolves a relative path against its own
    // working directory, and an absolute one has to survive being pasted into a resource script,
    // where a Windows path's backslashes are escapes.
    let icon_copy = out.join("ADI.ico");
    if let Err(e) = fs::copy(&icon, &icon_copy) {
        println!("cargo:warning=could not stage the icon ({e}) — ADI.exe will have no icon");
        return;
    }

    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    // The four-part binary version Windows shows in the file's properties. `ADI_VERSION` is the
    // git tag the release is cut from (scripts/version.sh), the same value the binaries compile
    // in, so the properties sheet and `adi-mono --version` cannot disagree.
    let display = env::var("ADI_VERSION").unwrap_or(version);
    println!("cargo:rerun-if-env-changed=ADI_VERSION");
    let mut parts: Vec<u16> = display
        .split(['.', '-'])
        .map_while(|p| p.parse().ok())
        .collect();
    parts.resize(4, 0);
    let quad = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);

    fs::write(out.join("ADI.manifest"), MANIFEST).expect("write manifest");
    let rc = format!(
        r#"#include <winuser.h>
1 ICON "ADI.ico"
1 24 "ADI.manifest"

1 VERSIONINFO
FILEVERSION {quad}
PRODUCTVERSION {quad}
FILEOS 0x40004L
FILETYPE 0x1L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "ADI"
      VALUE "FileDescription", "ADI"
      VALUE "FileVersion", "{display}"
      VALUE "InternalName", "ADI"
      VALUE "OriginalFilename", "ADI.exe"
      VALUE "ProductName", "ADI"
      VALUE "ProductVersion", "{display}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    fs::write(out.join("ADI.rc"), rc).expect("write rc");

    let object = out.join("ADI.res.o");
    let status = Command::new(&windres)
        .current_dir(&out)
        .args(["--input", "ADI.rc", "--output-format", "coff", "--output"])
        .arg(&object)
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg-bins={}", object.display());
        }
        Ok(s) => println!(
            "cargo:warning={} exited {s} — ADI.exe will have no icon",
            windres.display()
        ),
        Err(e) => println!("cargo:warning=could not run {}: {e}", windres.display()),
    }
}

/// The `windres` that matches the target ABI: mingw's own when cross-compiling (it is named after
/// the target triple), a bare `windres` when building on Windows under the gnu toolchain.
fn find_windres() -> Option<PathBuf> {
    if let Some(explicit) = env::var_os("WINDRES") {
        return Some(PathBuf::from(explicit));
    }
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("gnu") {
        return None; // MSVC wants rc.exe, which nothing that ships is built with.
    }
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "x86_64".into());
    [format!("{arch}-w64-mingw32-windres"), "windres".into()]
        .into_iter()
        .find(|c| runnable(Path::new(c)))
        .map(PathBuf::from)
}

fn runnable(command: &Path) -> bool {
    Command::new(command)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// `asInvoker` so opening ADI never raises a UAC prompt — the one privileged step it can take
/// (the `.adi` DNS route) elevates itself, and only when asked for. `longPathAware` because the
/// store lives under the user's profile and a project path there can outrun `MAX_PATH`.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="ADI" version="1.0.0.0"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#;
