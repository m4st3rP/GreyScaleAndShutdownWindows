//! Entry point.  Creates a hidden message-only window, installs a system-tray
//! icon, starts a background scheduler thread, and runs the Win32 message loop.
//!
//! Tray right-click menu
//! ─────────────────────
//!   Enable Greyscale Now
//!   Disable Greyscale
//!   ─────────────────────
//!   Edit Config in Notepad   (double-clicking the icon also opens it)
//!   Reload Config
//!   ─────────────────────
//!   Exit
//!
//! Config file
//! ───────────
//!   %APPDATA%\grayscale-timer\config.json  (created with defaults on first run)

#![windows_subsystem = "windows"]

mod config;
mod grayscale;

use std::{
    collections::HashSet,
    mem::{self, size_of},
    process::Command,
    sync::{Arc, Mutex, OnceLock},
    thread,
    time::Duration,
};

use chrono::{Local, NaiveDate, NaiveTime};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::CreateMutexW,
        },
        UI::{
            Shell::{
                Shell_NotifyIconW, NOTIFYICONDATAW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP,
                NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
                DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW,
                PostMessageW, PostQuitMessage, RegisterClassExW, SetForegroundWindow,
                TrackPopupMenu, TranslateMessage, HMENU, IDI_APPLICATION, MF_SEPARATOR,
                MF_STRING, MSG, TPM_BOTTOMALIGN, TPM_LEFTALIGN, WINDOW_EX_STYLE,
                WINDOW_STYLE, WM_APP, WM_COMMAND, WM_DESTROY, WM_LBUTTONDBLCLK, WM_RBUTTONUP,
                WNDCLASSEXW,
            },
        },
    },
};

use config::Config;

// ── Custom window messages ────────────────────────────────────────────────────
const WM_TRAY_ICON:        u32 = WM_APP + 1;
const WM_DO_GRAYSCALE:     u32 = WM_APP + 2;
const WM_DO_GRAYSCALE_OFF: u32 = WM_APP + 3;
const WM_DO_SHUTDOWN:      u32 = WM_APP + 4;
const WM_DO_NOTIFY:        u32 = WM_APP + 5;

// ── Context-menu command IDs ──────────────────────────────────────────────────
const IDM_GRAY_ON:  usize = 101;
const IDM_GRAY_OFF: usize = 102;
const IDM_EDIT:     usize = 103;
const IDM_RELOAD:   usize = 104;
const IDM_EXIT:     usize = 105;

const TRAY_UID: u32 = 1;

// ── App state (shared between main thread and scheduler) ─────────────────────
struct State {
    config:               Config,
    fired_grayscale:      Option<NaiveDate>,
    fired_grayscale_off:  Option<NaiveDate>,
    fired_shutdown:       Option<NaiveDate>,
    /// Keys are `"YYYY-MM-DD-<minutes_before>"` to fire each notification once.
    fired_notifs:    HashSet<String>,
    /// Message to show in the next WM_DO_NOTIFY balloon.
    notif_msg:       String,
}

static APP: OnceLock<Arc<Mutex<State>>> = OnceLock::new();

fn app() -> &'static Arc<Mutex<State>> {
    APP.get().expect("APP not initialised")
}

// ── HWND wrapper that is Send so we can move it into the scheduler thread ─────
struct SendHwnd(isize);
unsafe impl Send for SendHwnd {}

// ── UTF-16 helpers ────────────────────────────────────────────────────────────

fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn copy_wstr(dst: &mut [u16], src: &str) {
    let v: Vec<u16> = src.encode_utf16().collect();
    let n = v.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&v[..n]);
    dst[n] = 0;
}

// ── Tray-icon helpers ─────────────────────────────────────────────────────────

/// Zero-initialised NOTIFYICONDATAW with the mandatory fields set.
unsafe fn base_nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = mem::zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd   = hwnd;
    nid.uID    = TRAY_UID;
    nid
}

unsafe fn tray_add(hwnd: HWND) {
    let hicon = LoadIconW(HINSTANCE::default(), IDI_APPLICATION).unwrap_or_default();
    let mut nid = base_nid(hwnd);
    nid.uFlags           = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY_ICON;
    nid.hIcon            = hicon;
    copy_wstr(&mut nid.szTip, "Greyscale Timer");
    let _ = Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn tray_remove(hwnd: HWND) {
    let nid = base_nid(hwnd);
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

/// Show a balloon-tip notification from the tray icon.
unsafe fn balloon(hwnd: HWND, title: &str, body: &str) {
    let mut nid = base_nid(hwnd);
    nid.uFlags      = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;
    copy_wstr(&mut nid.szInfoTitle, title);
    copy_wstr(&mut nid.szInfo, body);
    let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
}

/// Show the right-click context menu at the current cursor position.
unsafe fn show_menu(hwnd: HWND) {
    // Keep the Vec<u16> alive until TrackPopupMenu returns (it blocks).
    let s_on    = wstr("Enable Greyscale Now");
    let s_off   = wstr("Disable Greyscale");
    let s_edit  = wstr("Edit Config in Notepad   (or double-click icon)");
    let s_rel   = wstr("Reload Config");
    let s_exit  = wstr("Exit");

    let hmenu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => return,
    };

    let _ = AppendMenuW(hmenu, MF_STRING,    IDM_GRAY_ON,  PCWSTR(s_on.as_ptr()));
    let _ = AppendMenuW(hmenu, MF_STRING,    IDM_GRAY_OFF, PCWSTR(s_off.as_ptr()));
    let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0,            PCWSTR::null());
    let _ = AppendMenuW(hmenu, MF_STRING,    IDM_EDIT,     PCWSTR(s_edit.as_ptr()));
    let _ = AppendMenuW(hmenu, MF_STRING,    IDM_RELOAD,   PCWSTR(s_rel.as_ptr()));
    let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0,            PCWSTR::null());
    let _ = AppendMenuW(hmenu, MF_STRING,    IDM_EXIT,     PCWSTR(s_exit.as_ptr()));

    let mut pt = POINT { x: 0, y: 0 };
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd); // required so the menu dismisses on click-away
    let _ = TrackPopupMenu(
        hmenu,
        TPM_BOTTOMALIGN | TPM_LEFTALIGN,
        pt.x, pt.y, 0,
        hwnd,
        None,
    );
    let _ = DestroyMenu(hmenu);
    // s_on … s_exit are dropped here, after the menu is gone → no dangling pointers
}

// ── Actions ───────────────────────────────────────────────────────────────────

fn open_config() {
    let _ = Command::new("notepad.exe")
        .arg(Config::config_path())
        .spawn();
}

fn reload_config(hwnd: HWND) {
    app().lock().unwrap().config = Config::load();
    unsafe { balloon(hwnd, "Greyscale Timer", "Config reloaded.") };
}

/// Initiate an OS shutdown with a 60-second grace window.
fn do_shutdown() {
    // /s = shutdown, /f = force-close apps, /t 60 = 60-second countdown
    let _ = Command::new("shutdown")
        .args(["/s", "/f", "/t", "60"])
        .spawn();
}

// ── Window procedure ──────────────────────────────────────────────────────────

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TRAY_ICON => {
                let event = (lp.0 as u32) & 0xFFFF;
                if event == WM_RBUTTONUP {
                    show_menu(hwnd);
                } else if event == WM_LBUTTONDBLCLK {
                    open_config();
                }
            }

            WM_COMMAND => {
                match (wp.0 & 0xFFFF) as usize {
                    IDM_GRAY_ON  => grayscale::set_grayscale(true),
                    IDM_GRAY_OFF => grayscale::set_grayscale(false),
                    IDM_EDIT     => open_config(),
                    IDM_RELOAD   => reload_config(hwnd),
                    IDM_EXIT     => {
                        tray_remove(hwnd);
                        PostQuitMessage(0);
                    }
                    _ => {}
                }
            }

            WM_DO_GRAYSCALE => {
                grayscale::set_grayscale(true);
                balloon(hwnd, "Greyscale Timer", "Greyscale colour filter enabled.");
            }

            WM_DO_GRAYSCALE_OFF => {
                grayscale::set_grayscale(false);
                balloon(hwnd, "Greyscale Timer", "Greyscale colour filter disabled.");
            }

            WM_DO_SHUTDOWN => {
                balloon(hwnd, "Greyscale Timer", "Shutting down in 60 seconds…");
                do_shutdown();
            }

            WM_DO_NOTIFY => {
                let msg_text = app().lock().unwrap().notif_msg.clone();
                balloon(hwnd, "Shutdown Warning", &msg_text);
            }

            WM_DESTROY => PostQuitMessage(0),

            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        }
        LRESULT(0)
    }
}

// ── Scheduler (background thread) ────────────────────────────────────────────
//
// Wakes every 15 seconds, compares the current HH:MM to configured times, and
// posts a custom message to the main window when a scheduled event fires.
// Each event is tracked by date so it fires at most once per day.

fn start_scheduler(hwnd: HWND) {
    let sh = SendHwnd(hwnd.0 as isize);

    thread::spawn(move || {
        // Reconstruct HWND from the stored raw value.
        let hwnd = HWND(sh.0 as *mut core::ffi::c_void);

        loop {
            thread::sleep(Duration::from_secs(15));

            let now   = Local::now();
            let today = now.date_naive();
            let hm    = now.format("%H:%M").to_string(); // e.g. "22:00"

            let cfg = app().lock().unwrap().config.clone();

            // ── Greyscale timer ────────────────────────────────────────────
            if cfg.grayscale_enabled {
                if cfg.grayscale_time == hm {
                    let already = app().lock().unwrap().fired_grayscale == Some(today);
                    if !already {
                        app().lock().unwrap().fired_grayscale = Some(today);
                        unsafe {
                            let _ = PostMessageW(hwnd, WM_DO_GRAYSCALE, WPARAM(0), LPARAM(0));
                        }
                    }
                }
                if cfg.grayscale_disable_time == hm {
                    let already = app().lock().unwrap().fired_grayscale_off == Some(today);
                    if !already {
                        app().lock().unwrap().fired_grayscale_off = Some(today);
                        unsafe {
                            let _ = PostMessageW(hwnd, WM_DO_GRAYSCALE_OFF, WPARAM(0), LPARAM(0));
                        }
                    }
                }
            }

            // ── Shutdown notifications & shutdown ──────────────────────────
            if cfg.shutdown_enabled {
                if let Ok(sd_t) = NaiveTime::parse_from_str(&cfg.shutdown_time, "%H:%M") {

                    // Pre-shutdown balloon notifications
                    for notif in &cfg.notifications {
                        let fire_t = sd_t
                            - chrono::Duration::minutes(notif.minutes_before as i64);
                        let fire_hm = fire_t.format("%H:%M").to_string();

                        if hm == fire_hm {
                            let key = format!("{}-{}", today, notif.minutes_before);
                            let already = app().lock().unwrap().fired_notifs.contains(&key);
                            if !already {
                                {
                                    let mut state = app().lock().unwrap();
                                    state.fired_notifs.insert(key);
                                    state.notif_msg = notif.message.clone();
                                }
                                unsafe {
                                    let _ = PostMessageW(
                                        hwnd, WM_DO_NOTIFY, WPARAM(0), LPARAM(0),
                                    );
                                }
                            }
                        }
                    }

                    // Shutdown itself
                    if cfg.shutdown_time == hm {
                        let already = app().lock().unwrap().fired_shutdown == Some(today);
                        if !already {
                            app().lock().unwrap().fired_shutdown = Some(today);
                            unsafe {
                                let _ = PostMessageW(
                                    hwnd, WM_DO_SHUTDOWN, WPARAM(0), LPARAM(0),
                                );
                            }
                        }
                    }
                }
            }
        }
    });
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    // ── Single-instance guard via named kernel mutex ───────────────────────
    let _mutex_guard;
    unsafe {
        let name = wstr("Local\\GrayscaleTimerSingleInstance");
        match CreateMutexW(None, true, PCWSTR(name.as_ptr())) {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    return; // another instance already running
                }
                _mutex_guard = h; // keep the handle alive for the process lifetime
            }
            Err(_) => {} // continue even if mutex creation failed
        }
    }

    // ── Load config, initialise global state ──────────────────────────────
    let cfg = Config::load();
    let activate_now = cfg.activate_on_start;

    APP.set(Arc::new(Mutex::new(State {
        config:               cfg,
        fired_grayscale:      None,
        fired_grayscale_off:  None,
        fired_shutdown:       None,
        fired_notifs:         HashSet::new(),
        notif_msg:            String::new(),
    }))).ok();

    if activate_now {
        grayscale::set_grayscale(true);
    }

    unsafe {
        // ── Register window class ──────────────────────────────────────────
        let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let class_name = wstr("GrayscaleTimerMsgWnd");

        let wc = WNDCLASSEXW {
            cbSize:        size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc:   Some(wnd_proc),
            hInstance:     HINSTANCE(hinstance.0 as *mut _),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..mem::zeroed()
        };
        let _ = RegisterClassExW(&wc);

        // ── Create hidden message-only window ──────────────────────────────
        // HWND_MESSAGE (-3) as parent → no visible window, no taskbar entry.
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wstr("Greyscale Timer").as_ptr()),
            WINDOW_STYLE(0),
            0, 0, 1, 1,
            HWND(-3isize as *mut _), // HWND_MESSAGE
            HMENU(std::ptr::null_mut()),
            HINSTANCE(hinstance.0 as *mut _),
            None,
        ) {
            Ok(h) => h,
            Err(_) => return,
        };

        tray_add(hwnd);
        start_scheduler(hwnd);

        // ── Message loop ───────────────────────────────────────────────────
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
