//! Activate / deactivate the Windows greyscale colour filter.
//!
//! WHY SendInput?
//! ──────────────
//! The previous approach wrote to the registry and then called
//! `SendNotifyMessageW(HWND_BROADCAST, WM_SETTINGCHANGE, …)`.  On Windows 11
//! the colour-filter host (dwm / colorcnv.dll) no longer reliably wakes up
//! from that broadcast.
//!
//! The most reliable way to toggle the filter is therefore to let Windows do
//! what it does when the user presses Win+Ctrl+C: we simulate exactly that
//! key sequence via `SendInput`.  We first write `FilterType = 0` (Greyscale)
//! to the registry so the filter comes up in the right mode, then we only
//! simulate the keypress when a toggle is actually needed (to keep the call
//! idempotent).

use std::{mem::size_of, thread, time::Duration};

use winreg::{enums::HKEY_CURRENT_USER, RegKey};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL, VK_LWIN,
};

/// Virtual-key code for the letter C (not defined in windows-rs by name).
const VK_C: VIRTUAL_KEY = VIRTUAL_KEY(0x43);

/// Enable (`true`) or disable (`false`) the greyscale colour filter.
///
/// Reads the current `Active` registry value first, so the call is idempotent:
/// calling `set_grayscale(true)` while the filter is already on is a no-op.
pub fn set_grayscale(enable: bool) {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    if let Ok((key, _)) = hkcu.create_subkey(r"Software\Microsoft\ColorFiltering") {
        // Pin the filter type to Grayscale (0) and enable the keyboard shortcut.
        let _ = key.set_value("FilterType", &0u32);
        let _ = key.set_value("HotKeyActivationEnabled", &1u32);
        let _ = key.set_value("HotkeyEnabled", &1u32);

        let active: u32 = key.get_value("Active").unwrap_or(0);
        let is_currently_enabled = active != 0;

        if enable != is_currently_enabled {
            // Brief pause so the registry writes are visible before the keypress
            // is processed.
            thread::sleep(Duration::from_millis(100));

            simulate_win_ctrl_c();
        }
    }
}

/// Synthesise a Win + Ctrl + C key chord via `SendInput`.
fn simulate_win_ctrl_c() {
    let inputs = [
        make_key(VK_LWIN,    false),
        make_key(VK_CONTROL, false),
        make_key(VK_C,       false),
        make_key(VK_C,       true),
        make_key(VK_CONTROL, true),
        make_key(VK_LWIN,    true),
    ];
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

fn make_key(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk:         vk,
                wScan:       0,
                dwFlags:     if key_up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time:        0,
                dwExtraInfo: 0,
            },
        },
    }
}
