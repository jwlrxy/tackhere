// Modified for TackHere by jwlrxy (2026).
//! autostart — "Start in the system tray at sign-in", kept where Windows itself reads
//! it: the per-user Run key. The registry is the single source of truth (the
//! installer can switch autostart on too, so a flag in pinned.json would
//! drift out of step).

use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows::core::{w, HSTRING};

fn value_name() -> HSTRING {
    HSTRING::from(format!("TackHere{}", crate::instance_suffix()))
}

const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

fn registered_command() -> Option<String> {
    let name = value_name();
    let mut size = 0u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, &name, RRF_RT_REG_SZ, None, None, Some(&mut size)).ok().ok()?;
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, &name, RRF_RT_REG_SZ, None,
            Some(buffer.as_mut_ptr().cast()), Some(&mut size)).ok().ok()?;
        String::from_utf16(&buffer[..buffer.iter().position(|&c| c == 0)?]).ok()
    }
}

fn launch_command() -> Option<String> {
    let executable = std::env::current_exe().ok()?;
    Some(format!("\"{}\" --minimized", executable.display()))
}

pub fn is_enabled() -> bool {
    launch_command().zip(registered_command()).is_some_and(|(expected, actual)| {
        actual.eq_ignore_ascii_case(&expected)
    })
}

/// Keep a previously enabled setting working when the portable EXE moves.
pub fn initialize(first_launch: bool) {
    if !is_enabled() && (first_launch || registered_command().is_some()) && !set_enabled(true) {
        crate::logfile::warning!("Could not register the silent sign-in command");
    }
}

/// Launch hidden in the tray at login, not with the window open.
pub fn set_enabled(enabled: bool) -> bool {
    unsafe {
        if !enabled {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, &value_name());
            return r.is_ok() || r == ERROR_FILE_NOT_FOUND;
        }
        let Some(command) = launch_command() else { return false };
        let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            &value_name(),
            REG_SZ.0,
            Some(wide.as_ptr() as *const _),
            (wide.len() * 2) as u32,
        )
        .is_ok() && is_enabled()
    }
}
