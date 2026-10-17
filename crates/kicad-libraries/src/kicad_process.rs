//! Whether KiCad has been running since a moment: KiCad reads the
//! `.kicad_dbl` only when it loads the library, so after the groups or
//! fields change, a KiCad that started earlier needs a restart. Windows
//! lists running programs and when each started; only read here.

/// The programs that load the database library.
const KICAD_PROGRAMS: [&str; 3] = ["kicad.exe", "eeschema.exe", "pcbnew.exe"];

/// Whether a KiCad that started before `at_ms` (milliseconds since 1970,
/// UTC) is running now, or `None` when this system cannot tell.
#[cfg(windows)]
pub fn started_before(at_ms: u64) -> Option<bool> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // FILETIME counts 100 ns from 1601; Unix time counts from 1970.
    const UNIX_EPOCH_AS_FILETIME: u64 = 116_444_736_000_000_000;
    // SAFETY: the snapshot handle is checked and closed; the entry's size
    // is set before the first call, as Process32FirstW needs.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more && !found {
            let length = entry
                .szExeFile
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..length]).to_lowercase();
            if KICAD_PROGRAMS.contains(&name.as_str()) {
                let process =
                    OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID);
                if process.is_null() {
                    // Cannot tell when it started: assume before.
                    found = true;
                } else {
                    let mut times: [FILETIME; 4] = std::mem::zeroed();
                    let [created, exited, kernel, user] = &mut times;
                    if GetProcessTimes(process, created, exited, kernel, user) != 0 {
                        let ticks = (u64::from(created.dwHighDateTime) << 32)
                            | u64::from(created.dwLowDateTime);
                        let started_ms = ticks.saturating_sub(UNIX_EPOCH_AS_FILETIME) / 10_000;
                        found = started_ms < at_ms;
                    } else {
                        found = true;
                    }
                    CloseHandle(process);
                }
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        Some(found)
    }
}

#[cfg(not(windows))]
pub fn started_before(_at_ms: u64) -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn nothing_started_before_1970() {
        assert_eq!(super::started_before(0), Some(false));
    }
}
