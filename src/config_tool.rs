use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::Graphics::Gdi::*;
use windows::core::PCWSTR;
use std::mem::size_of;
use crate::config::Config;

fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn run() {
    unsafe {
        let instance = GetModuleHandleW(None).unwrap();
        let class_name = wstr("GrayscaleTimerConfigWnd");

        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0 as *mut _),
            ..Default::default()
        };

        RegisterClassExW(&wc);

        let _hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wstr("Grayscale Timer Config").as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT, CW_USEDEFAULT, 400, 500,
            None, None, instance, None,
        ).unwrap();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                create_controls(hwnd);
            }
            WM_COMMAND => {
                let id = (wp.0 & 0xFFFF) as i32;
                if id == 1001 { // Save
                    save_config(hwnd);
                } else if id == 1002 { // Install
                    install_service();
                } else if id == 1003 { // Uninstall
                    uninstall_service();
                }
            }
            WM_DESTROY => PostQuitMessage(0),
            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        }
        LRESULT(0)
    }
}

fn create_controls(hwnd: HWND) {
    let cfg = Config::load();
    unsafe {
        let instance = GetModuleHandleW(None).unwrap();

        CreateWindowExW(Default::default(), PCWSTR(wstr("STATIC").as_ptr()), PCWSTR(wstr("Grayscale Time (HH:MM):").as_ptr()), WS_CHILD | WS_VISIBLE, 10, 10, 200, 20, hwnd, HMENU(std::ptr::null_mut()), instance, None);
        CreateWindowExW(WS_EX_CLIENTEDGE, PCWSTR(wstr("EDIT").as_ptr()), PCWSTR(wstr(&cfg.grayscale_time).as_ptr()), WS_CHILD | WS_VISIBLE | WS_BORDER, 220, 10, 100, 20, hwnd, HMENU(101 as *mut _), instance, None).unwrap();

        CreateWindowExW(Default::default(), PCWSTR(wstr("STATIC").as_ptr()), PCWSTR(wstr("Disable Time (HH:MM):").as_ptr()), WS_CHILD | WS_VISIBLE, 10, 40, 200, 20, hwnd, HMENU(std::ptr::null_mut()), instance, None);
        CreateWindowExW(WS_EX_CLIENTEDGE, PCWSTR(wstr("EDIT").as_ptr()), PCWSTR(wstr(&cfg.grayscale_disable_time).as_ptr()), WS_CHILD | WS_VISIBLE | WS_BORDER, 220, 40, 100, 20, hwnd, HMENU(102 as *mut _), instance, None).unwrap();

        CreateWindowExW(Default::default(), PCWSTR(wstr("BUTTON").as_ptr()), PCWSTR(wstr("Grayscale Enabled").as_ptr()), WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32), 10, 70, 200, 20, hwnd, HMENU(103 as *mut _), instance, None).unwrap();
        if cfg.grayscale_enabled {
            CheckDlgButton(hwnd, 103, BST_CHECKED);
        }

        CreateWindowExW(Default::default(), PCWSTR(wstr("STATIC").as_ptr()), PCWSTR(wstr("Shutdown Time (HH:MM):").as_ptr()), WS_CHILD | WS_VISIBLE, 10, 100, 200, 20, hwnd, HMENU(std::ptr::null_mut()), instance, None);
        CreateWindowExW(WS_EX_CLIENTEDGE, PCWSTR(wstr("EDIT").as_ptr()), PCWSTR(wstr(&cfg.shutdown_time).as_ptr()), WS_CHILD | WS_VISIBLE | WS_BORDER, 220, 100, 100, 20, hwnd, HMENU(104 as *mut _), instance, None).unwrap();

        CreateWindowExW(Default::default(), PCWSTR(wstr("BUTTON").as_ptr()), PCWSTR(wstr("Shutdown Enabled").as_ptr()), WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32), 10, 130, 200, 20, hwnd, HMENU(105 as *mut _), instance, None).unwrap();
        if cfg.shutdown_enabled {
            CheckDlgButton(hwnd, 105, BST_CHECKED);
        }

        CreateWindowExW(Default::default(), PCWSTR(wstr("BUTTON").as_ptr()), PCWSTR(wstr("Save Config").as_ptr()), WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32), 10, 200, 100, 30, hwnd, HMENU(1001 as *mut _), instance, None).unwrap();
        CreateWindowExW(Default::default(), PCWSTR(wstr("BUTTON").as_ptr()), PCWSTR(wstr("Install Service").as_ptr()), WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32), 120, 200, 120, 30, hwnd, HMENU(1002 as *mut _), instance, None).unwrap();
        CreateWindowExW(Default::default(), PCWSTR(wstr("BUTTON").as_ptr()), PCWSTR(wstr("Uninstall Service").as_ptr()), WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32), 250, 200, 130, 30, hwnd, HMENU(1003 as *mut _), instance, None).unwrap();
    }
}

fn get_window_text(hwnd: HWND, id: i32) -> String {
    unsafe {
        let child = GetDlgItem(hwnd, id).unwrap();
        let len = GetWindowTextLengthW(child);
        let mut buf = vec![0u16; len as usize + 1];
        GetWindowTextW(child, &mut buf);
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

fn save_config(hwnd: HWND) {
    let mut cfg = Config::load();
    cfg.grayscale_time = get_window_text(hwnd, 101);
    cfg.grayscale_disable_time = get_window_text(hwnd, 102);
    cfg.grayscale_enabled = unsafe { IsDlgButtonChecked(hwnd, 103) == BST_CHECKED.0 as u32 };
    cfg.shutdown_time = get_window_text(hwnd, 104);
    cfg.shutdown_enabled = unsafe { IsDlgButtonChecked(hwnd, 105) == BST_CHECKED.0 as u32 };
    cfg.save();
    unsafe { MessageBoxW(hwnd, PCWSTR(wstr("Config saved.").as_ptr()), PCWSTR(wstr("Success").as_ptr()), MB_OK) };
}

fn install_service() {
    let exe_path = std::env::current_exe().unwrap();
    let exe_str = exe_path.to_str().unwrap();
    let bin_path = format!("\"{}\" service", exe_str);

    let _ = std::process::Command::new("sc.exe")
        .args(["create", "TheWorldIsGreyShutItWin", &format!("binPath= {}", bin_path), "start=auto"])
        .status();
    let _ = std::process::Command::new("sc.exe")
        .args(["description", "TheWorldIsGreyShutItWin", "Greyscale & shutdown scheduler"])
        .status();
    let _ = std::process::Command::new("sc.exe")
        .args(["start", "TheWorldIsGreyShutItWin"])
        .status();
}

fn uninstall_service() {
    let _ = std::process::Command::new("sc.exe")
        .args(["stop", "TheWorldIsGreyShutItWin"])
        .status();
    let _ = std::process::Command::new("sc.exe")
        .args(["delete", "TheWorldIsGreyShutItWin"])
        .status();
}
