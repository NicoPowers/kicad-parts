//! Windowless child processes that cannot outlive the test process.

use crate::{ServiceResult, error};
use std::fs::{self, File};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

/// A long-running service process. Dropping it kills the process; if the
/// test process dies first, the job object kills it instead.
pub struct ServiceProcess {
    name: String,
    child: Child,
    log: PathBuf,
    _job: job::Job,
}

impl ServiceProcess {
    /// Starts `command` with no console window and no stdin, writes its
    /// stdout and stderr to `log`, and puts it in a kill-on-close job that
    /// allows no child processes, so the service cannot open a browser or
    /// any other program.
    pub fn spawn(name: &str, mut command: Command, log: &Path) -> ServiceResult<Self> {
        let out = File::create(log)
            .map_err(|io| error(format!("cannot create `{}`: {io}", log.display())))?;
        let err = out
            .try_clone()
            .map_err(|io| error(format!("cannot share `{}`: {io}", log.display())))?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err));
        let (child, job) = spawn_in_job(name, &mut command, true)?;
        Ok(Self {
            name: name.to_owned(),
            child,
            log: log.to_owned(),
            _job: job,
        })
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// The exit status if the process has already ended.
    pub fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// The end of the service log, for error messages.
    pub fn log_tail(&self) -> String {
        let text = fs::read_to_string(&self.log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(30)..].join("\n")
    }

    pub fn log_path(&self) -> &Path {
        &self.log
    }

    /// Kills the process and waits for it, so its files are released.
    pub fn stop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for ServiceProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Runs a short command to completion with no console window, inside a
/// kill-on-close job, and captures its output.
pub fn run_hidden(name: &str, command: &mut Command) -> ServiceResult<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Build tools start compilers, so short commands may have children;
    // they still get no window and die with the job.
    let (child, _job) = spawn_in_job(name, command, false)?;
    child
        .wait_with_output()
        .map_err(|io| error(format!("waiting for {name} failed: {io}")))
}

/// A port that was free on 127.0.0.1 a moment ago. Callers retry with a
/// new port if a service then fails to bind it.
pub fn free_loopback_port() -> ServiceResult<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|io| error(format!("cannot reserve a loopback port: {io}")))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|io| error(format!("cannot read the reserved port: {io}")))
}

fn spawn_in_job(
    name: &str,
    command: &mut Command,
    single_process: bool,
) -> ServiceResult<(Child, job::Job)> {
    job::hide_window(command);
    let job = job::Job::new(single_process)?;
    let mut child = command
        .spawn()
        .map_err(|io| error(format!("cannot start {name}: {io}")))?;
    // The child starts suspended, so it is in the job (and under its
    // limits) before it runs a single instruction.
    if let Err(failure) = job.attach(&child).and_then(|()| job::resume(&child)) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(failure);
    }
    Ok((child, job))
}

#[cfg(windows)]
mod job {
    use crate::{ServiceResult, error};
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};
    use std::ptr::null;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
    };

    /// A job object whose processes all die when its last handle closes,
    /// including when this process is killed.
    pub struct Job(HANDLE);

    // SAFETY: a job handle is a kernel object handle, usable from any thread.
    unsafe impl Send for Job {}
    // SAFETY: as above; the handle is only read after creation.
    unsafe impl Sync for Job {}

    /// No console window, and suspended until [`resume`].
    pub fn hide_window(command: &mut Command) {
        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }

    /// Resumes the threads of a child started suspended (only its main
    /// thread exists at that point).
    pub fn resume(child: &Child) -> ServiceResult<()> {
        let pid = child.id();
        // SAFETY: a snapshot of all threads; closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(error(format!(
                "CreateToolhelp32Snapshot failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        // SAFETY: zeroed is valid for this C struct; dwSize is set before use.
        let mut entry: THREADENTRY32 = unsafe { zeroed() };
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut resumed = 0;
        // SAFETY: `entry` is a valid, sized out structure.
        let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: opening a thread of our own suspended child.
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if !thread.is_null() {
                    // SAFETY: a valid thread handle, closed right after.
                    unsafe {
                        ResumeThread(thread);
                        CloseHandle(thread);
                    }
                    resumed += 1;
                }
            }
            // SAFETY: as above.
            more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
        }
        // SAFETY: the snapshot handle from above, closed once.
        unsafe { CloseHandle(snapshot) };
        if resumed == 0 {
            return Err(error(format!("cannot resume process {pid}")));
        }
        Ok(())
    }

    impl Job {
        /// With `single_process`, the job holds only the process attached
        /// to it: any attempt to start a child fails.
        pub fn new(single_process: bool) -> ServiceResult<Self> {
            // SAFETY: an unnamed job with default security.
            let handle = unsafe { CreateJobObjectW(null(), null()) };
            if handle.is_null() {
                return Err(error(format!(
                    "CreateJobObjectW failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            let job = Self(handle);
            // SAFETY: zeroed is a valid initial value for this plain C struct.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if single_process {
                limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
                limits.BasicLimitInformation.ActiveProcessLimit = 1;
            }
            // SAFETY: the pointer and size describe `limits`.
            let set = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if set == 0 {
                return Err(error(format!(
                    "SetInformationJobObject failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            Ok(job)
        }

        pub fn attach(&self, child: &Child) -> ServiceResult<()> {
            // SAFETY: valid job and process handles.
            let assigned =
                unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) };
            if assigned == 0 {
                return Err(error(format!(
                    "AssignProcessToJobObject failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            Ok(())
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateJobObjectW and is closed
            // once; closing it kills anything still in the job.
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(not(windows))]
mod job {
    use crate::ServiceResult;
    use std::process::{Child, Command};

    /// Outside Windows the services are direct children that
    /// `ServiceProcess` kills on drop.
    pub struct Job;

    pub fn hide_window(_command: &mut Command) {}

    pub fn resume(_child: &Child) -> ServiceResult<()> {
        Ok(())
    }

    impl Job {
        pub fn new(_single_process: bool) -> ServiceResult<Self> {
            Ok(Self)
        }

        pub fn attach(&self, _child: &Child) -> ServiceResult<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_ports_are_nonzero() {
        assert_ne!(free_loopback_port().unwrap(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn hidden_commands_run_and_report_output() {
        let mut command = Command::new("cmd");
        command.args(["/C", "echo native-services"]);
        let output = run_hidden("cmd", &mut command).unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("native-services"));
    }

    #[cfg(windows)]
    #[test]
    fn services_cannot_start_child_processes() {
        use std::os::windows::process::CommandExt;
        // cmd itself runs, but the program it tries to start is refused, as
        // a browser launch by a service would be.
        let temporary = tempfile::tempdir().unwrap();
        let log = temporary.path().join("cmd.log");
        let marker = temporary.path().join("child-ran");
        let mut command = Command::new("cmd");
        command.raw_arg(format!("/C cmd /C mkdir \"{}\"", marker.display()));
        let mut service = ServiceProcess::spawn("cmd", command, &log).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while service.exited().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(service.exited().is_some());
        assert!(!marker.exists(), "a service started a child process");
        // Control: the same command without the limit does start its child.
        let mut command = Command::new("cmd");
        command.raw_arg(format!("/C cmd /C mkdir \"{}\"", marker.display()));
        assert!(run_hidden("cmd", &mut command).unwrap().status.success());
        assert!(marker.exists());
    }

    #[cfg(windows)]
    #[test]
    fn dropping_a_service_ends_its_process() {
        let temporary = tempfile::tempdir().unwrap();
        let mut command = Command::new("ping");
        command.args(["-n", "60", "127.0.0.1"]);
        let mut service =
            ServiceProcess::spawn("ping", command, &temporary.path().join("ping.log")).unwrap();
        assert!(service.exited().is_none());
        service.stop();
        assert!(service.exited().is_some());
    }
}
