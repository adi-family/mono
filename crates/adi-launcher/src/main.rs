//! `ADI.exe` — the app a person opens on Windows.
//!
//! The whole point of this binary is that there is exactly one thing to click. macOS gets that
//! for free: a `.app` is a folder, so the four executables the platform is made of sit inside it
//! where nobody has to choose between them. Windows has no such envelope, so the package used to
//! be four `.exe` files in one folder next to two `.cmd` files, and the first question it asked a
//! new person was "which of these am I supposed to run?" — a question no one should be asked.
//!
//! The answer is the installer (`apps/windows/installer/adi.nsi`), which puts the executables in
//! a `bin\` directory nobody browses, and this — the one entry in the Start menu. It does what
//! opening `ADI.app` does: starts the stack, waits for it, opens the control panel, and then
//! lives in the tray as the platform's face.
//!
//! It holds no logic of its own. Everything it knows comes from `adi-mono status --json` and
//! everything it does is `adi-mono <args>`, exactly as the macOS app does through `Core.swift`.

// Off Windows nothing calls into it but the tests, which is the point of compiling it there.
#[cfg_attr(not(windows), allow(dead_code))]
mod cli;

#[cfg(windows)]
mod tray;

#[cfg(windows)]
fn main() {
    tray::main();
}

/// Built on macOS and Linux as a plain, harmless binary so `cargo build`, `cargo clippy` and
/// `cargo test` cover this crate on the machine it is developed on — the Windows package is
/// cross-compiled from exactly such a machine, and a crate that only compiles on the target is a
/// crate whose breakage is found by CI instead of by the person writing it.
#[cfg(not(windows))]
fn main() {
    eprintln!(
        "ADI's launcher is the Windows front end for the {} CLI; on this platform, use {} \
         directly (or, on macOS, open ADI.app).",
        cli::BINARY,
        cli::BINARY
    );
    std::process::exit(1);
}
