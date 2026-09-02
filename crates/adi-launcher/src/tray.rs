//! The tray app itself: a hidden window, an icon by the clock, and a menu.
//!
//! This is the Windows answer to `ADI.app` — the one thing a person opens. Opening it does what
//! opening the macOS app does, in the same order: bring the stack up (`adi-mono up`, idempotent,
//! a no-op on a machine that is already running), wait until something actually answers, then
//! open the control panel in the browser. It then stays in the tray so the stack has a face:
//! what is running, and the four things you can do to it.
//!
//! There is no window on purpose. Everything a person looks at is the control panel, which is a
//! web page; a second, native window would only be a worse copy of it. What the tray adds is
//! what a web page cannot do — being there before the services are, and telling you they are up.
//!
//! Win32 directly rather than through a GUI framework: an icon, a menu and a message loop are
//! about two hundred lines of C API, and a framework would be a megabyte and a build-system
//! dependency for the same thing.

// Every unsafe block here is one documented user32/shell32 entry point. The pattern is the one
// the platform specifies: a zeroed struct whose `cbSize` names its version, a window procedure
// with the exact signature `WNDPROC` requires, and pointers that live until the call returns.
#![allow(unsafe_code)]
// Handles are pointers and the API's integers are C integers: the casts between them are the
// price of speaking to the C API at all, and hiding each one behind a helper would obscure
// rather than clarify.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::ptr_as_ptr
)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use std::{ptr, thread};

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, Shell_NotifyIconW, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    FindWindowW, GetCursorPos, GetMessageW, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTSIZE,
    LR_LOADFROMFILE, LR_SHARED, LoadImageW, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SW_SHOWNORMAL,
    SetForegroundWindow, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP, WM_COMMAND,
    WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};

use crate::cli::{self, Report};

// MARK: the messages this window answers to

/// The tray icon's callback. `lparam` carries the mouse message.
const WM_TRAY: u32 = WM_APP + 1;
/// A status poll landed: repaint the tooltip.
const WM_REFRESH: u32 = WM_APP + 2;
/// Someone asked for the control panel — the startup thread, or a second copy of ADI that found
/// this one already running and handed its click over rather than starting a rival tray icon.
const WM_SHOW_PANEL: u32 = WM_APP + 3;
/// Say something in a balloon; `wparam` picks which.
const WM_BALLOON: u32 = WM_APP + 4;

const BALLOON_ROUTE: usize = 1;
const BALLOON_STUCK: usize = 2;

// MARK: menu items

const ID_OPEN: usize = 0x100;
const ID_START: usize = 0x101;
const ID_STOP: usize = 0x102;
const ID_ROUTE: usize = 0x103;
const ID_QUIT: usize = 0x1ff;

/// The window class, which is also how a second copy finds the first.
const CLASS_NAME: &str = "AdiLauncherWindow";
/// Per-user, so two people signed into the same machine each get their own ADI.
const MUTEX_NAME: &str = "Local\\ADI.Launcher.Single";

/// How long to wait for the stack to answer before giving up on opening the panel. Generous for
/// the same reason the macOS app's is: a cold start binds ports, reads the store, and on a fresh
/// machine registers four scheduled tasks first.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// How often to ask the core what is true. The macOS window polls at the same rate.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

// MARK: process-wide state
//
// A window procedure is a bare `extern "system" fn`, so anything it reads has to be reachable
// without a receiver. Three statics and one mutex is the whole of it.

static WINDOW: AtomicIsize = AtomicIsize::new(0);
static ICON: AtomicIsize = AtomicIsize::new(0);
/// `TaskbarCreated`, broadcast when Explorer restarts. The icon has to be added again or it is
/// simply gone for the rest of the session — the app looks like it quit.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// The last good status report. Never cleared by a failed poll: one CLI invocation that could not
/// run is not the same as ADI being off, and blinking between the two would make the tooltip a
/// liar twice a second.
fn state() -> MutexGuard<'static, Report> {
    static STATE: OnceLock<Mutex<Report>> = OnceLock::new();
    let mutex = STATE.get_or_init(|| Mutex::new(Report::default()));
    // A panicking worker must not take the tray down with it: the report is plain data, so the
    // worst a poisoned lock holds is a slightly stale one.
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `adi-mono --version`, read once. Shown at the top of the menu, which is the only place a
/// person can find out what they are running without opening a terminal.
fn version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        let (status, output) = cli::run(&["--version"]);
        if status != 0 {
            return String::new();
        }
        // `adi-mono 0.3.1` — the last whitespace-separated word is the version.
        output
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .to_string()
    })
}

// MARK: entry point

pub fn main() {
    // One tray icon per user. A second launch is not an error and must not be a second icon: it
    // is someone clicking ADI again because they want the panel, so hand that to the copy that
    // is already running and get out of its way.
    if already_running() {
        let existing = unsafe { FindWindowW(wide(CLASS_NAME).as_ptr(), ptr::null()) };
        if !existing.is_null() {
            unsafe { PostMessageW(existing, WM_SHOW_PANEL, 0, 0) };
            return;
        }
        // A stale mutex with no window behind it (the previous copy was killed): carry on and
        // become the tray ourselves rather than exiting into nothing.
    }

    let hwnd = unsafe { create_window() };
    if hwnd.is_null() {
        return;
    }
    WINDOW.store(hwnd as isize, Ordering::SeqCst);
    unsafe { add_icon(hwnd) };

    start_stack(hwnd as isize);
    poll_forever(hwnd as isize);

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    // `> 0`: GetMessageW returns 0 on WM_QUIT and -1 on error, and treating -1 as a message is
    // the classic way to spin a message loop at 100% CPU forever.
    while unsafe { GetMessageW(&raw mut msg, ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
}

/// Whether another copy of the launcher holds the single-instance mutex.
fn already_running() -> bool {
    // Deliberately leaked: the handle must live as long as the process, and dropping it would
    // release the name the moment this function returns.
    let handle = unsafe { CreateMutexW(ptr::null(), 1, wide(MUTEX_NAME).as_ptr()) };
    !handle.is_null() && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS
}

/// A top-level window that is never shown.
///
/// Not a message-only window (`HWND_MESSAGE`), tempting as that is: those do not receive
/// broadcasts, and `TaskbarCreated` — the one that puts the icon back after Explorer restarts —
/// is a broadcast.
unsafe fn create_window() -> HWND {
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    let class = wide(CLASS_NAME);
    let mut wc: WNDCLASSW = unsafe { std::mem::zeroed() };
    wc.lpfnWndProc = Some(wndproc);
    wc.hInstance = instance;
    wc.lpszClassName = class.as_ptr();
    unsafe { RegisterClassW(&raw const wc) };

    TASKBAR_CREATED.store(
        unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
        Ordering::SeqCst,
    );

    unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            wide("ADI").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == TASKBAR_CREATED.load(Ordering::SeqCst) && msg != 0 {
        unsafe { add_icon(hwnd) };
        return 0;
    }
    match msg {
        WM_TRAY => {
            match lparam as u32 {
                // Clicking the icon means "take me to ADI", the same as clicking the app.
                WM_LBUTTONUP => open_panel(),
                WM_RBUTTONUP => unsafe { show_menu(hwnd) },
                _ => {}
            }
            0
        }
        WM_COMMAND => {
            command(hwnd, wparam & 0xffff);
            0
        }
        WM_REFRESH => {
            unsafe { update_tip(hwnd) };
            0
        }
        WM_SHOW_PANEL => {
            open_panel();
            0
        }
        WM_BALLOON => {
            unsafe { balloon(hwnd, wparam) };
            0
        }
        WM_DESTROY => {
            unsafe { remove_icon(hwnd) };
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Act on a menu choice.
///
/// Every action is a thread, because every action is a CLI invocation that can take seconds:
/// `up` registers and starts four scheduled tasks, and `dns install-route` sits on a UAC prompt
/// for as long as the person takes to answer it. Doing any of that inline would freeze the menu.
fn command(hwnd: HWND, id: usize) {
    let raw = hwnd as isize;
    match id {
        ID_OPEN => open_panel(),
        ID_START => perform(raw, &["up"]),
        ID_STOP => perform(raw, &["disable"]),
        ID_ROUTE => perform(raw, &["dns", "install-route"]),
        ID_QUIT => unsafe {
            // Only the tray goes away. The services are scheduled tasks and stay up — quitting
            // the launcher is closing a window, not stopping the machine's platform. "Stop ADI"
            // is the menu item that stops things, and it says so.
            remove_icon(hwnd);
            PostQuitMessage(0);
        },
        _ => {}
    }
}

/// Run `adi-mono <args>` off the message loop, then republish the status.
fn perform(hwnd: isize, args: &'static [&'static str]) {
    thread::spawn(move || {
        let _ = cli::run(args);
        refresh(hwnd);
    });
}

/// Bring the stack up and go to the panel — what opening the app means.
///
/// Runs on every launch. `up` is idempotent and never restarts a running service, so on a machine
/// that is already up this is a status read and a browser tab; on a fresh one it is the whole
/// install.
fn start_stack(hwnd: isize) {
    thread::spawn(move || {
        let _ = cli::run(&["up"]);
        let deadline = Instant::now() + START_TIMEOUT;
        // Waiting for something to actually answer, rather than opening the moment `up` returns:
        // Task Scheduler accepting a task is not the same as the front door serving, and a
        // browser tab that fails to load looks like ADI is broken.
        loop {
            refresh(hwnd);
            if state().any_running || Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(400));
        }
        let report = state().clone();
        if report.any_running {
            open_url(&report.dashboard_url());
            if !report.setup.dns_route {
                post(hwnd, WM_BALLOON, BALLOON_ROUTE);
            }
        } else {
            post(hwnd, WM_BALLOON, BALLOON_STUCK);
        }
    });
}

/// Ask the core what is true, twice a second-ish, forever.
fn poll_forever(hwnd: isize) {
    thread::spawn(move || {
        loop {
            thread::sleep(POLL_INTERVAL);
            refresh(hwnd);
        }
    });
}

/// One status read, published to the tray.
fn refresh(hwnd: isize) {
    if let Some(report) = cli::report() {
        *state() = report;
        post(hwnd, WM_REFRESH, 0);
    }
}

fn post(hwnd: isize, msg: u32, wparam: usize) {
    if hwnd != 0 {
        unsafe { PostMessageW(hwnd as HWND, msg, wparam, 0) };
    }
}

/// Open the control panel, starting ADI first if nothing is up.
///
/// The macOS app's `openDashboard`, verbatim in intent: pressing this always means "take me to
/// the dashboard", so when nothing is running it means start everything and then go there.
fn open_panel() {
    let hwnd = WINDOW.load(Ordering::SeqCst);
    thread::spawn(move || {
        if !state().any_running {
            let _ = cli::run(&["enable"]);
            let deadline = Instant::now() + START_TIMEOUT;
            while {
                refresh(hwnd);
                !state().any_running && Instant::now() < deadline
            } {
                thread::sleep(Duration::from_millis(400));
            }
        }
        let url = state().dashboard_url();
        open_url(&url);
    });
}

fn open_url(url: &str) {
    unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            wide("open").as_ptr(),
            wide(url).as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

// MARK: the icon and the menu

/// The icon, in order of preference: the one compiled into this executable (right size for every
/// DPI), the `ADI.ico` installed beside it (there even if the resource compiler was missing at
/// build time), and finally the system's default so the tray is never empty.
unsafe fn icon() -> isize {
    let cached = ICON.load(Ordering::SeqCst);
    if cached != 0 {
        return cached;
    }
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    // MAKEINTRESOURCE(1) — build.rs names the icon resource `1`. The macro is a small integer
    // *in place of* a string pointer, which is exactly what `without_provenance` spells.
    let embedded = unsafe {
        LoadImageW(
            instance,
            ptr::without_provenance(1),
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTSIZE | LR_SHARED,
        )
    };
    let handle = if embedded.is_null() {
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("ADI.ico")))
            .unwrap_or_default();
        let from_file = unsafe {
            LoadImageW(
                ptr::null_mut(),
                wide(&beside.to_string_lossy()).as_ptr(),
                IMAGE_ICON,
                0,
                0,
                LR_DEFAULTSIZE | LR_LOADFROMFILE,
            )
        };
        if from_file.is_null() {
            unsafe {
                LoadImageW(
                    ptr::null_mut(),
                    IDI_APPLICATION,
                    IMAGE_ICON,
                    0,
                    0,
                    LR_DEFAULTSIZE | LR_SHARED,
                )
            }
        } else {
            from_file
        }
    } else {
        embedded
    };
    ICON.store(handle as isize, Ordering::SeqCst);
    handle as isize
}

/// A zeroed `NOTIFYICONDATAW` addressed at our one icon.
unsafe fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

unsafe fn add_icon(hwnd: HWND) {
    let mut nid = unsafe { icon_data(hwnd) };
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = unsafe { icon() } as _;
    fill(&mut nid.szTip, &tip());
    unsafe { Shell_NotifyIconW(NIM_ADD, &raw const nid) };
}

unsafe fn remove_icon(hwnd: HWND) {
    let nid = unsafe { icon_data(hwnd) };
    unsafe { Shell_NotifyIconW(NIM_DELETE, &raw const nid) };
}

unsafe fn update_tip(hwnd: HWND) {
    let mut nid = unsafe { icon_data(hwnd) };
    nid.uFlags = NIF_TIP;
    fill(&mut nid.szTip, &tip());
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &raw const nid) };
}

unsafe fn balloon(hwnd: HWND, which: usize) {
    let (title, body) = match which {
        BALLOON_ROUTE => (
            "ADI is running",
            "To reach it at http://app.adi/ as well, choose \"Set up the .adi domain\" from this \
             icon. It asks for admin once.",
        ),
        BALLOON_STUCK => (
            "ADI did not start",
            "The services were asked to start but nothing is answering yet. Try \"Start ADI\" \
             from this icon.",
        ),
        _ => return,
    };
    let mut nid = unsafe { icon_data(hwnd) };
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;
    fill(&mut nid.szInfoTitle, title);
    fill(&mut nid.szInfo, body);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &raw const nid) };
}

/// What hovering the icon says.
fn tip() -> String {
    let report = state();
    let what = if report.any_running {
        "Running"
    } else if report.any_enabled() {
        "Starting…"
    } else {
        "Off"
    };
    format!("ADI — {what}")
}

unsafe fn show_menu(hwnd: HWND) {
    let report = state().clone();
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return;
    }

    let heading = if version().is_empty() {
        tip()
    } else {
        format!("{} ({})", tip(), version())
    };
    unsafe {
        AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, wide(&heading).as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
        AppendMenuW(
            menu,
            MF_STRING,
            ID_OPEN,
            wide("Open control panel").as_ptr(),
        );

        if report.any_enabled() {
            AppendMenuW(menu, MF_STRING, ID_STOP, wide("Stop ADI").as_ptr());
        } else {
            AppendMenuW(menu, MF_STRING, ID_START, wide("Start ADI").as_ptr());
        }

        // Checked and unclickable once it is done: the state is worth showing, and re-running an
        // install that is already in place would spend a UAC prompt to change nothing.
        if report.setup.dns_route {
            AppendMenuW(
                menu,
                MF_STRING | MF_CHECKED | MF_GRAYED,
                ID_ROUTE,
                wide("The .adi domain is set up").as_ptr(),
            );
        } else {
            AppendMenuW(
                menu,
                MF_STRING,
                ID_ROUTE,
                wide("Set up the .adi domain…").as_ptr(),
            );
        }

        AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
        AppendMenuW(
            menu,
            MF_STRING,
            ID_QUIT,
            wide("Quit (services keep running)").as_ptr(),
        );

        let mut point = POINT { x: 0, y: 0 };
        GetCursorPos(&raw mut point);
        // Without this the menu does not dismiss when the person clicks away from it — it is the
        // documented workaround, and the trailing WM_NULL is the other half of it.
        SetForegroundWindow(hwnd);
        TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            hwnd,
            ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
    }
}

// MARK: strings

/// A NUL-terminated UTF-16 copy, which is what every `…W` entry point takes.
fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Copy `text` into one of `NOTIFYICONDATAW`'s fixed buffers, truncated to fit and always
/// NUL-terminated — the struct's fields are arrays, and the shell reads them as C strings.
fn fill(dest: &mut [u16], text: &str) {
    let source = wide(text);
    let room = dest
        .len()
        .saturating_sub(1)
        .min(source.len().saturating_sub(1));
    dest[..room].copy_from_slice(&source[..room]);
    dest[room] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The UTF-16 of an ASCII string, which is what these buffers should end up holding.
    fn utf16(text: &str) -> Vec<u16> {
        text.bytes().map(u16::from).collect()
    }

    #[test]
    fn fill_truncates_and_terminates() {
        let mut buffer = [0xffffu16; 5];
        fill(&mut buffer, "abcdefgh");
        assert_eq!(&buffer[..4], utf16("abcd").as_slice());
        assert_eq!(buffer[4], 0, "the last cell must be the terminator");
    }

    #[test]
    fn fill_keeps_a_short_string_whole() {
        let mut buffer = [0xffffu16; 8];
        fill(&mut buffer, "ADI");
        assert_eq!(&buffer[..3], utf16("ADI").as_slice());
        assert_eq!(buffer[3], 0);
    }

    #[test]
    fn wide_is_nul_terminated() {
        assert_eq!(wide("hi"), [utf16("hi"), vec![0]].concat());
    }
}
