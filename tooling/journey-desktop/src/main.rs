//! `journey-desktop PROGRAM [ARGS...]` runs PROGRAM on a new, hidden Win32
//! desktop (a desktop object in the interactive window station, not a Task
//! View virtual desktop) and exits with its exit code.
//!
//! Every process PROGRAM starts inherits that desktop, so tauri-driver,
//! msedgedriver, the Tauri app and WebView2 draw where nobody can see them
//! and never take focus on the user's desktop. All of them run in a job that
//! is killed when this helper exits, and the desktop closes with them.
//!
//! The helper also watches the process that started it (the journey
//! runner). If that process dies, even by `taskkill /F`, the helper kills
//! the job, so no invisible tauri-driver is left holding its port.

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: journey-desktop PROGRAM [ARGS...]");
        return std::process::ExitCode::from(2);
    }
    match hidden::run(&args) {
        // Forward the full 32-bit exit code (Windows codes exceed 255).
        Ok(code) => std::process::exit(code as i32),
        Err(message) => {
            eprintln!("journey-desktop: {message}");
            std::process::ExitCode::from(1)
        }
    }
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!(
        "journey-desktop: hidden desktops are a Windows feature; run journeys with --visible"
    );
    std::process::ExitCode::from(2)
}

/// Quotes one argument the way `CommandLineToArgvW` and the MSVC runtime
/// parse it back.
#[cfg_attr(not(windows), allow(dead_code))]
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut quoted = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(backslashes));
                quoted.push(c);
                backslashes = 0;
            }
        }
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(windows)]
mod hidden {
    use super::quote_arg;
    use std::mem::{size_of, zeroed};
    use std::ptr::null;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::JobObjects::TerminateJobObject;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, CreateDesktopW};
    use windows_sys::Win32::System::StationsAndDesktops::{
        DESKTOP_CREATEMENU, DESKTOP_CREATEWINDOW, DESKTOP_ENUMERATE, DESKTOP_READOBJECTS,
        DESKTOP_WRITEOBJECTS,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess, INFINITE, OpenProcess,
        PROCESS_INFORMATION, PROCESS_SYNCHRONIZE, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOW,
        TerminateProcess, WaitForMultipleObjects,
    };

    /// Rights on our handle to the desktop: what creating and using windows
    /// on it requires. Processes started on it open it with their own rights.
    const DESKTOP_ACCESS: u32 = DESKTOP_CREATEWINDOW
        | DESKTOP_CREATEMENU
        | DESKTOP_READOBJECTS
        | DESKTOP_WRITEOBJECTS
        | DESKTOP_ENUMERATE;

    /// The process that started this one, from a process snapshot.
    fn parent_process_id() -> Option<u32> {
        let me = std::process::id();
        // SAFETY: a snapshot of all processes; closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return None;
        }
        let _snapshot = Owned(snapshot, CloseHandle);
        // SAFETY: zeroed is valid for this C struct; dwSize is set before use.
        let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        // SAFETY: `entry` is a valid, sized out structure.
        let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
        while more {
            if entry.th32ProcessID == me {
                return Some(entry.th32ParentProcessID);
            }
            // SAFETY: as above.
            more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
        }
        None
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn last_error(what: &str) -> String {
        format!("{what} failed: {}", std::io::Error::last_os_error())
    }

    /// Closes a handle when dropped.
    struct Owned(
        HANDLE,
        unsafe extern "system" fn(HANDLE) -> windows_sys::core::BOOL,
    );

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: the handle came from the matching Create call and is
            // closed exactly once.
            unsafe { (self.1)(self.0) };
        }
    }

    pub fn run(args: &[String]) -> Result<u32, String> {
        let parent_id =
            parent_process_id().ok_or_else(|| "cannot find the parent process".to_owned())?;
        // SAFETY: opening a process for waiting only.
        let parent = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, parent_id) };
        if parent.is_null() {
            return Err(last_error("OpenProcess(parent)"));
        }
        let _parent = Owned(parent, CloseHandle);

        let name = format!("kicad-parts-journey-{}", std::process::id());
        // SAFETY: plain Win32 call with a NUL-terminated name and no
        // optional structures.
        let desktop = unsafe {
            CreateDesktopW(
                wide(&name).as_ptr(),
                null(),
                null(),
                0,
                DESKTOP_ACCESS,
                null(),
            )
        };
        if desktop.is_null() {
            return Err(last_error("CreateDesktopW"));
        }
        let _desktop = Owned(desktop, CloseDesktop);

        // SAFETY: an unnamed job with default security.
        let job = unsafe { CreateJobObjectW(null(), null()) };
        if job.is_null() {
            return Err(last_error("CreateJobObjectW"));
        }
        let _job = Owned(job, CloseHandle);
        // SAFETY: zeroed is a valid initial value for this plain C struct.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the pointer and size describe `limits`.
        let set = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if set == 0 {
            return Err(last_error("SetInformationJobObject"));
        }

        let mut desktop_path = wide(&format!("winsta0\\{name}"));
        let mut command_line = wide(
            &args
                .iter()
                .map(|arg| quote_arg(arg))
                .collect::<Vec<_>>()
                .join(" "),
        );
        // SAFETY: zeroed is a valid initial value for these C structs.
        let mut startup: STARTUPINFOW = unsafe { zeroed() };
        startup.cb = size_of::<STARTUPINFOW>() as u32;
        startup.lpDesktop = desktop_path.as_mut_ptr();
        startup.dwFlags = STARTF_USESTDHANDLES;
        // SAFETY: reading this process's standard handles.
        unsafe {
            startup.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
            startup.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
            startup.hStdError = GetStdHandle(STD_ERROR_HANDLE);
        }
        let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
        // SAFETY: all pointers are valid for the call; the command line
        // buffer is mutable as CreateProcessW requires. The child starts
        // suspended so it is in the job before it can start anything.
        let created = unsafe {
            CreateProcessW(
                null(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_SUSPENDED,
                null(),
                null(),
                &startup,
                &mut process,
            )
        };
        if created == 0 {
            return Err(last_error(&format!("starting {}", args[0])));
        }
        let _process = Owned(process.hProcess, CloseHandle);
        let _thread = Owned(process.hThread, CloseHandle);
        // SAFETY: valid job and process handles.
        if unsafe { AssignProcessToJobObject(job, process.hProcess) } == 0 {
            let error = last_error("AssignProcessToJobObject");
            // SAFETY: the child is still suspended and outside the job, so
            // nothing else would end it.
            unsafe { TerminateProcess(process.hProcess, 1) };
            return Err(error);
        }
        // SAFETY: the suspended main thread of the child.
        unsafe { ResumeThread(process.hThread) };
        let handles = [process.hProcess, parent];
        // SAFETY: two valid handles we own; wait for whichever ends first.
        let woke = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
        if woke == WAIT_OBJECT_0 + 1 {
            // SAFETY: a valid job handle; ends every process in it.
            unsafe { TerminateJobObject(job, 1) };
            return Err(
                "the process that started journey-desktop exited; stopped the journey processes"
                    .into(),
            );
        }
        let mut code = 1u32;
        // SAFETY: `code` is a valid out pointer.
        unsafe { GetExitCodeProcess(process.hProcess, &mut code) };
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::quote_arg;

    #[test]
    fn quotes_arguments_for_the_windows_command_line() {
        assert_eq!(quote_arg("--port"), "--port");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(
            quote_arg(r"C:\Program Files\x.exe"),
            r#""C:\Program Files\x.exe""#
        );
        assert_eq!(quote_arg(r#"a "b""#), r#""a \"b\"""#);
        assert_eq!(quote_arg(r"C:\dir with space\"), r#""C:\dir with space\\""#);
    }
}
