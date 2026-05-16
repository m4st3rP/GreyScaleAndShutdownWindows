//! Entry point.  Creates a hidden top-level window, installs a system-tray
//! icon, starts a background scheduler thread, and runs the Win32 message loop.
//!
//! Tray menu (opens on click)
//! ──────────────────────────
//!   Enable Greyscale Now
//!   Disable Greyscale
//!   ─────────────────────
//!   Edit Config in Notepad
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

use chrono::{Local, NaiveDate, NaiveDateTime, NaiveTime};
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
                Shell_NotifyIconW, NIN_SELECT, NOTIFYICONDATAW, NIF_ICON,
                NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
                NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
                DispatchMessageW, GetCursorPos, GetMessageW, LoadCursorW, LoadIconW,
                PostMessageW, PostQuitMessage, RegisterClassExW, SetForegroundWindow,
                TrackPopupMenuEx, TranslateMessage, HMENU, IDC_ARROW, IDI_APPLICATION,
                MF_SEPARATOR, MF_STRING, MSG, SW_HIDE, ShowWindow, TPM_LEFTALIGN,
                TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WM_APP, WM_COMMAND,
                WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NULL,
                WM_RBUTTONUP, WM_USER, WNDCLASSEXW,
            },
        },
    },
};

use config::Config;


// ── Custom window messages ────────────────────────────────────────────────────
const WM_TRAY_ICON:        u32 = WM_APP + 1;
const NIN_KEYSELECT:       u32 = WM_USER + 1;
const WM_DO_GRAYSCALE:     u32 = WM_APP + 2;
const WM_DO_GRAYSCALE_OFF: u32 = WM_APP + 3;
const WM_DO_SHUTDOWN:      u32 = WM_APP + 4;
const WM_DO_NOTIFY:        u32 = WM_APP + 5;

// ── Context-menu command IDs ──────────────────────────────────────────────────
const IDM_SNOOZE:   usize = 100;
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
    snoozed_today:        Option<NaiveDate>,
    snooze_at:            Option<NaiveDateTime>,
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

/// Returns true if `now` is between `start` and `end`, handling midnight wrap-around.
fn is_time_in_range(now: NaiveTime, start: NaiveTime, end: NaiveTime) -> bool {
    if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

fn copy_wstr(dst: &mut [u16], src: &str) {
    let v: Vec<u16> = src.encode_utf16().collect();
    let n = v.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&v[..n]);
    dst[n] = 0;
}

// ── Tray-icon helpers ─────────────────────────────────────────────────────────

/// Zero-initialised NOTIFYICONDATAW with the mandatory fields set.
fn base_nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { mem::zeroed() };
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd   = hwnd;
    nid.uID    = TRAY_UID;
    nid
}

fn tray_add(hwnd: HWND) {
    let hicon = unsafe { LoadIconW(HINSTANCE::default(), IDI_APPLICATION).unwrap_or_default() };
    let mut nid = base_nid(hwnd);
    nid.uFlags           = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY_ICON;
    nid.hIcon            = hicon;
    copy_wstr(&mut nid.szTip, "Greyscale Timer");

    unsafe {
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
}

fn tray_remove(hwnd: HWND) {
    let nid = base_nid(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

/// Show a balloon-tip notification from the tray icon.
fn balloon(hwnd: HWND, title: &str, body: &str) {
    let mut nid = base_nid(hwnd);
    nid.uFlags      = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;
    copy_wstr(&mut nid.szInfoTitle, title);
    copy_wstr(&mut nid.szInfo, body);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Show the right-click context menu at the current cursor position.
/// Returns the (date, original_time) of the shutdown that is currently relevant
/// for snoozing (within the notification window or just fired).
fn get_relevant_shutdown(state: &State) -> Option<(NaiveDate, NaiveTime)> {
    let cfg = &state.config;
    if !cfg.shutdown_enabled {
        return None;
    }

    let now = Local::now();
    let sd_time = NaiveTime::parse_from_str(&cfg.shutdown_time, "%H:%M").ok()?;

    // Check if we are within the snooze window before the scheduled time.
    // We consider yesterday, today, and tomorrow in case the shutdown is near midnight.
    let today = now.date_naive();
    for offset in [-1, 0, 1] {
        let date = today + chrono::Duration::days(offset);
        let dt = date.and_time(sd_time);
        let diff = dt.signed_duration_since(now.naive_local());

        // If we're within the snooze window before shutdown, OR
        // we're within 60 seconds AFTER the scheduled time (the grace period).
        if diff.num_seconds() >= -60 && diff.num_minutes() <= cfg.snooze_available_minutes_before as i64 {
             return Some((date, sd_time));
        }
    }
    None
}

fn handle_command(hwnd: HWND, id: usize) {
    match id {
        IDM_SNOOZE => {
            let mut state = app().lock().unwrap();
            if let Some((date, time)) = get_relevant_shutdown(&state) {
                state.snoozed_today = Some(date);
                state.snooze_at = Some(date.and_time(time) + chrono::Duration::minutes(15));
                state.fired_shutdown = Some(date);
                let _ = Command::new("shutdown").arg("/a").spawn();
                balloon(hwnd, "Greyscale Timer", "Shutdown snoozed for 15 minutes.");
            }
        }
        IDM_GRAY_ON  => grayscale::set_grayscale(true),
        IDM_GRAY_OFF => grayscale::set_grayscale(false),
        IDM_EDIT     => open_config(),
        IDM_RELOAD   => reload_config(hwnd),
        IDM_EXIT     => {
            tray_remove(hwnd);
            unsafe {
                PostQuitMessage(0);
            }
        }
        _ => {}
    }
}

fn show_menu(hwnd: HWND) {
    let s_snooze = wstr("Snooze Shutdown (15 mins)");
    let s_on    = wstr("Enable Greyscale Now");
    let s_off   = wstr("Disable Greyscale");
    let s_edit  = wstr("Edit Config in Notepad");
    let s_rel   = wstr("Reload Config");
    let s_exit  = wstr("Exit");

    let hmenu = match unsafe { CreatePopupMenu() } {
        Ok(m) => m,
        Err(_) => return,
    };

    let can_snooze = {
        let state = app().lock().unwrap();
        if let Some((date, _)) = get_relevant_shutdown(&state) {
            state.snoozed_today != Some(date)
        } else {
            false
        }
    };

    unsafe {
        if can_snooze {
            let _ = AppendMenuW(hmenu, MF_STRING, IDM_SNOOZE, PCWSTR(s_snooze.as_ptr()));
            let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
        }

        let _ = AppendMenuW(hmenu, MF_STRING,    IDM_GRAY_ON,  PCWSTR(s_on.as_ptr()));
        let _ = AppendMenuW(hmenu, MF_STRING,    IDM_GRAY_OFF, PCWSTR(s_off.as_ptr()));
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0,            PCWSTR::null());
        let _ = AppendMenuW(hmenu, MF_STRING,    IDM_EDIT,     PCWSTR(s_edit.as_ptr()));
        let _ = AppendMenuW(hmenu, MF_STRING,    IDM_RELOAD,   PCWSTR(s_rel.as_ptr()));
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0,            PCWSTR::null());
        let _ = AppendMenuW(hmenu, MF_STRING,    IDM_EXIT,     PCWSTR(s_exit.as_ptr()));

        let mut pt = POINT { x: 0, y: 0 };
        let _ = GetCursorPos(&mut pt);

        // Microsoft KB135610: To have TrackPopupMenu function correctly,
        // we must set the window to the foreground before the call, and
        // post a WM_NULL message after.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenuEx(
            hmenu,
            (TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD).0,
            pt.x, pt.y,
            hwnd,
            None,
        );
        if cmd.0 != 0 {
            handle_command(hwnd, cmd.0 as usize);
        }
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(hmenu);
    }
}

// ── Actions ───────────────────────────────────────────────────────────────────

fn open_config() {
    let _ = Command::new("notepad.exe")
        .arg(Config::config_path())
        .spawn();
}

fn reload_config(hwnd: HWND) {
    app().lock().unwrap().config = Config::load();
    balloon(hwnd, "Greyscale Timer", "Config reloaded.");
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
    match msg {
        WM_TRAY_ICON => {
            let event = (lp.0 as u32) & 0xFFFF;

            // Version 4 sends WM_CONTEXTMENU for right-click/Menu key,
            // and NIN_SELECT/NIN_KEYSELECT for left-click/Enter.
            if event == WM_RBUTTONUP || event == WM_LBUTTONUP || event == WM_LBUTTONDBLCLK
                || event == (WM_CONTEXTMENU as u16) as u32
                || event == NIN_SELECT || event == NIN_KEYSELECT
            {
                show_menu(hwnd);
            }
        }

        WM_COMMAND => {
            handle_command(hwnd, (wp.0 & 0xFFFF) as usize);
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

        WM_DESTROY => unsafe {
            PostQuitMessage(0);
        },

        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
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
                let current_time = now.time();
                let start_t = NaiveTime::parse_from_str(&cfg.grayscale_time, "%H:%M").unwrap_or_else(|_| NaiveTime::from_hms_opt(22, 0, 0).unwrap());
                let end_t = NaiveTime::parse_from_str(&cfg.grayscale_disable_time, "%H:%M").unwrap_or_else(|_| NaiveTime::from_hms_opt(7, 0, 0).unwrap());

                if is_time_in_range(current_time, start_t, end_t) {
                    let already = app().lock().unwrap().fired_grayscale == Some(today);
                    if !already {
                        app().lock().unwrap().fired_grayscale = Some(today);
                        app().lock().unwrap().fired_grayscale_off = None; // Reset so it can fire later
                        unsafe {
                            let _ = PostMessageW(hwnd, WM_DO_GRAYSCALE, WPARAM(0), LPARAM(0));
                        }
                    }
                } else {
                    let already = app().lock().unwrap().fired_grayscale_off == Some(today);
                    if !already {
                        app().lock().unwrap().fired_grayscale_off = Some(today);
                        app().lock().unwrap().fired_grayscale = None; // Reset so it can fire later
                        unsafe {
                            let _ = PostMessageW(hwnd, WM_DO_GRAYSCALE_OFF, WPARAM(0), LPARAM(0));
                        }
                    }
                }
            }

            // ── Shutdown notifications & shutdown ──────────────────────────
            if cfg.shutdown_enabled {
                // Check for snoozed shutdown
                let snooze_trigger = {
                    let mut state = app().lock().unwrap();
                    if let Some(snooze_dt) = state.snooze_at {
                        if now.naive_local() >= snooze_dt {
                            state.snooze_at = None;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };

                if snooze_trigger {
                    unsafe {
                        let _ = PostMessageW(hwnd, WM_DO_SHUTDOWN, WPARAM(0), LPARAM(0));
                    }
                }

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
    let name = wstr("Local\\GrayscaleTimerSingleInstance");
    unsafe {
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

    APP.set(Arc::new(Mutex::new(State {
        config:               cfg.clone(),
        fired_grayscale:      None,
        fired_grayscale_off:  None,
        fired_shutdown:       None,
        snoozed_today:        None,
        snooze_at:            None,
        fired_notifs:         HashSet::new(),
        notif_msg:            String::new(),
    }))).ok();

    if cfg.grayscale_enabled {
        let now = Local::now();
        let current_time = now.time();
        let today = now.date_naive();
        let start = NaiveTime::parse_from_str(&cfg.grayscale_time, "%H:%M").unwrap_or_else(|_| NaiveTime::from_hms_opt(22, 0, 0).unwrap());
        let end = NaiveTime::parse_from_str(&cfg.grayscale_disable_time, "%H:%M").unwrap_or_else(|_| NaiveTime::from_hms_opt(7, 0, 0).unwrap());

        if is_time_in_range(current_time, start, end) {
            grayscale::set_grayscale(true);
            app().lock().unwrap().fired_grayscale = Some(today);
        } else {
            grayscale::set_grayscale(false);
            app().lock().unwrap().fired_grayscale_off = Some(today);
        }
    }

    // ── Register window class ──────────────────────────────────────────
    let hinstance = unsafe { GetModuleHandleW(PCWSTR::null()).unwrap_or_default() };
    let class_name = wstr("GrayscaleTimerMsgWnd");

    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wnd_proc),
        hInstance: HINSTANCE(hinstance.0 as *mut _),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        ..unsafe { mem::zeroed() }
    };
    unsafe {
        let _ = RegisterClassExW(&wc);
    }

    // ── Create hidden top-level window ──────────────────────────────
    // Using WS_POPUP ensures it's a top-level window without decor.
    let hwnd = match unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wstr("Greyscale Timer").as_ptr()),
            windows::Win32::UI::WindowsAndMessaging::WS_POPUP,
            0,
            0,
            1,
            1,
            HWND(std::ptr::null_mut()),
            HMENU(std::ptr::null_mut()),
            HINSTANCE(hinstance.0 as *mut _),
            None,
        )
    } {
        Ok(h) => h,
        Err(_) => return,
    };

    tray_add(hwnd);
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }

    // Trigger initial balloon if needed
    let cfg = &app().lock().unwrap().config;
    if cfg.grayscale_enabled {
        let now = Local::now().time();
        let start = NaiveTime::parse_from_str(&cfg.grayscale_time, "%H:%M").unwrap_or_else(|_| NaiveTime::from_hms_opt(22, 0, 0).unwrap());
        let end = NaiveTime::parse_from_str(&cfg.grayscale_disable_time, "%H:%M").unwrap_or_else(|_| NaiveTime::from_hms_opt(7, 0, 0).unwrap());

        unsafe {
            if is_time_in_range(now, start, end) {
                let _ = PostMessageW(hwnd, WM_DO_GRAYSCALE, WPARAM(0), LPARAM(0));
            } else {
                // If it's disabled at startup, we usually don't need a balloon
                // saying "Greyscale disabled" unless it was previously on.
                // But the user said: "Even if the filter was already enabled...
                // you want the balloon notification to appear again if the app starts".
                // So we show it.
                let _ = PostMessageW(hwnd, WM_DO_GRAYSCALE_OFF, WPARAM(0), LPARAM(0));
            }
        }
    }

    start_scheduler(hwnd);

    // ── Message loop ───────────────────────────────────────────────────
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
