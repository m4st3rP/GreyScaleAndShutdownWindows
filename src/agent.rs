use std::{
    mem::{self, size_of},
};

use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM, INVALID_HANDLE_VALUE, CloseHandle},
    System::LibraryLoader::GetModuleHandleW,
    UI::Shell::{
        Shell_NotifyIconW, NOTIFYICONDATAW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP,
        NIIF_INFO, NIM_ADD, NIM_MODIFY,
    },
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, LoadIconW,
        PostQuitMessage, RegisterClassExW, HMENU, IDI_APPLICATION, MSG, WINDOW_EX_STYLE,
        WINDOW_STYLE, WM_APP, WM_DESTROY, WNDCLASSEXW,
    },
};
use windows::core::PCWSTR;
use crate::grayscale;
use crate::ipc::{Command as IpcCommand, PIPE_NAME};
use std::io::Read;
use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE, PIPE_WAIT};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND};
use std::fs::File;
use std::os::windows::io::FromRawHandle;
use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
use winreg::RegKey;

const WM_TRAY_ICON: u32 = WM_APP + 1;
const TRAY_UID: u32 = 1;

fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn copy_wstr(dst: &mut [u16], src: &str) {
    let v: Vec<u16> = src.encode_utf16().collect();
    let n = v.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&v[..n]);
    dst[n] = 0;
}

unsafe fn base_nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = mem::zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_UID;
    nid
}

unsafe fn tray_add(hwnd: HWND) {
    let hicon = LoadIconW(HINSTANCE::default(), IDI_APPLICATION).unwrap_or_default();
    let mut nid = base_nid(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY_ICON;
    nid.hIcon = hicon;
    copy_wstr(&mut nid.szTip, "Greyscale Timer Agent");
    let _ = Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn balloon(hwnd: HWND, title: &str, body: &str) {
    let mut nid = base_nid(hwnd);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;
    copy_wstr(&mut nid.szInfoTitle, title);
    copy_wstr(&mut nid.szInfo, body);
    let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_DESTROY => PostQuitMessage(0),
            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        }
        LRESULT(0)
    }
}

struct SendHwnd(isize);
unsafe impl Send for SendHwnd {}

pub fn run() {
    register_run_key();

    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let class_name = wstr("GrayscaleTimerAgentWnd");

        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: HINSTANCE(hinstance.0 as *mut _),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..mem::zeroed()
        };
        let _ = RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wstr("Greyscale Timer Agent").as_ptr()),
            WINDOW_STYLE(0),
            0, 0, 1, 1,
            HWND(-3isize as *mut _), // HWND_MESSAGE
            HMENU(std::ptr::null_mut()),
            HINSTANCE(hinstance.0 as *mut _),
            None,
        ).unwrap();

        tray_add(hwnd);

        let sh = SendHwnd(hwnd.0 as isize);
        std::thread::spawn(move || {
            let hwnd = HWND(sh.0 as *mut _);
            ipc_listener(hwnd);
        });

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).as_bool() {
            let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn ipc_listener(hwnd: HWND) {
    let pipe_name = wstr(PIPE_NAME);
    loop {
        unsafe {
            let h_pipe = CreateNamedPipeW(
                PCWSTR(pipe_name.as_ptr()),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                1,
                1024,
                1024,
                0,
                None,
            );

            if h_pipe != INVALID_HANDLE_VALUE {
                if ConnectNamedPipe(h_pipe, None).is_ok() {
                    let mut file = File::from_raw_handle(h_pipe.0);
                    let mut buffer = [0u8; 1024];
                    if let Ok(n) = file.read(&mut buffer) {
                        if let Ok(cmd) = serde_json::from_slice::<IpcCommand>(&buffer[..n]) {
                            match cmd {
                                IpcCommand::SetGrayscale(en) => {
                                    grayscale::set_grayscale(en);
                                }
                                IpcCommand::ShowNotification { title, message } => {
                                    balloon(hwnd, &title, &message);
                                }
                            }
                        }
                    }
                }
                let _ = CloseHandle(h_pipe);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

fn register_run_key() {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Run", KEY_SET_VALUE) {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(s) = exe_path.to_str() {
                let cmd = format!("\"{}\" agent", s);
                let _ = key.set_value("GrayscaleTimerAgent", &cmd);
            }
        }
    }
}
