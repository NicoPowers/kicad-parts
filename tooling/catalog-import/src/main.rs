//! `catalog-import local-team-server [--desktop | --json] [--team-code CODE]`
//!
//! Starts a disposable team server on 127.0.0.1 for trying the desktop app
//! by hand: a PocketBase with the starting categories, the parts of the
//! sample library (`fixtures/sample-library`) added as Add part adds them,
//! and three team members (two who can edit, one who can only read), each
//! with a new random password. It prints the server address and the
//! accounts, then:
//!
//! - with `--team-code MA`, gives the team that code, which names its KiCad
//!   libraries (MA-Parts); without it, someone who can edit chooses it in
//!   the desktop app;
//! - with `--desktop`, opens the built desktop app pointed at the server,
//!   with the sample library as its local library folder, and stops the
//!   server when the app closes;
//! - with `--json`, prints one JSON line instead of text and stops when
//!   standard input closes (the desktop journeys use this);
//! - otherwise, stops when you press Enter.
//!
//! The server runs without a window in a kill-on-close job, so it also
//! stops if this command is killed. Its data lives in
//! `target/local-team-server/run-<process id>` and is deleted when it stops,
//! including on Ctrl+C; a run killed outright is deleted by the next run.
//! Nothing else is written.
//!
//! `catalog-import kicad-libraries --team-code CODE FOLDER`
//! `catalog-import kicad-libraries --server URL --email EMAIL FOLDER`
//!
//! Writes the KiCad libraries the desktop app makes (kicad-parts-f397) into
//! `FOLDER/<team code>`, without the app: the database library with one
//! entry per part revision, and the symbols, footprints and 3D models it
//! uses. With `--server`, it reads that team server, signed in as `EMAIL`
//! with the password in the `KICAD_PARTS_PASSWORD` environment variable,
//! and refuses if the team has no code yet. Otherwise it starts a
//! disposable team server with the sample library's parts and the code
//! `--team-code` gives, as `local-team-server` does, and deletes it after
//! reading. Prints one JSON summary: entry counts, files written, problems,
//! and how long reading and writing took. Libraries from disposable team
//! servers count as one team's, so running it again updates the same
//! folder.

use catalog::pocketbase::{read_back, read_team, submit_team_code};
use catalog_import::libraries;
use catalog_import::run_folder::RunFolder;
use catalog_import::services::{self, Member};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, mpsc};
use std::time::Instant;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

const USAGE: &str = "usage: catalog-import local-team-server [--desktop | --json] [--team-code MA]\n       catalog-import kicad-libraries --team-code MA FOLDER\n       catalog-import kicad-libraries --server URL --email EMAIL FOLDER";

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("local-team-server") => local_team_server(&args[1..]),
        Some("kicad-libraries") => kicad_libraries(&args[1..]),
        _ => Err(USAGE.into()),
    }
}

#[derive(Debug, PartialEq)]
enum Then {
    WaitForEnter,
    OpenDesktop,
    PrintJson,
}

fn local_team_server(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (team_code, args) = team_code_option(args).ok_or(USAGE)?;
    let then = server_options(&args).ok_or(USAGE)?;
    let desktop = desktop_binary();
    if then == Then::OpenDesktop && !desktop.is_file() {
        return Err(format!(
            "{} is not built yet. Build it first:\n  trunk build              (in apps/desktop-ui)\n  cargo build --locked -p kicad-parts-desktop --features custom-protocol",
            desktop.display()
        )
        .into());
    }

    // A known folder under target/, not the system temp folder, so a run
    // that is killed outright is cleaned up by the next one.
    let root = services::repository_root();
    let folder = RunFolder::create(&root.join("target").join("local-team-server"))?;
    let (stop_sender, stop) = mpsc::channel::<Stop>();
    stop_on_ctrl_c(stop_sender.clone());
    let (server, admin, _superuser) = services::start(&root, folder.path())?;
    let seeded = sample_library::seed(&admin, &[])?;
    if let Some(code) = &team_code {
        submit_team_code(&admin, code)?;
    }
    let members = [
        services::add_member(&admin, "Local Editor", "editor@example.test", true)?,
        services::add_member(&admin, "Second Editor", "second.editor@example.test", true)?,
        services::add_member(&admin, "Read-only Member", "viewer@example.test", false)?,
    ];

    let mut out = std::io::stdout().lock();
    match then {
        Then::PrintJson => {
            let accounts: Vec<_> = members
                .iter()
                .map(|member| {
                    serde_json::json!({
                        "name": member.name,
                        "email": member.email,
                        "password": member.password,
                        "canEditCatalog": member.can_edit_catalog,
                    })
                })
                .collect();
            writeln!(
                out,
                "{}",
                serde_json::json!({
                    "url": server.url(),
                    "accounts": accounts,
                    "teamCode": team_code,
                })
            )?;
            out.flush()?;
            drop(out);
            // Stop when whoever started us closes our input.
            let sender = stop_sender.clone();
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines() {
                    if line.is_err() {
                        break;
                    }
                }
                let _ = sender.send(Stop::Finished);
            });
        }
        Then::WaitForEnter | Then::OpenDesktop => {
            writeln!(out, "Team server: {}", server.url())?;
            writeln!(
                out,
                "It has the starting categories and the sample library's {} parts (fixtures/sample-library).",
                seeded.parts.len()
            )?;
            writeln!(
                out,
                "Sign in to the desktop app with one of these accounts:"
            )?;
            for member in &members {
                writeln!(out, "{}", describe(member))?;
            }
            writeln!(
                out,
                "The passwords are new for this run and are not saved anywhere."
            )?;
            out.flush()?;
            drop(out);
            let sender = stop_sender.clone();
            if then == Then::WaitForEnter {
                // Enter stops the server (with --desktop, closing the app
                // does).
                let typed = sender.clone();
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let _ = std::io::stdin().lock().read_line(&mut line);
                    let _ = typed.send(Stop::Finished);
                });
            }
            if then == Then::OpenDesktop {
                println!("Opening the desktop app. Close it to stop the team server.");
                let mut command = Command::new(&desktop);
                command
                    .arg(format!("--team-server={}", server.url()))
                    .arg(format!("--team-email={}", members[0].email))
                    .arg(format!(
                        "--data-dir={}",
                        folder.path().join("desktop").display()
                    ))
                    .arg(format!(
                        "--catalog-root={}",
                        sample_library::root().display()
                    ));
                let mut child = command.spawn()?;
                std::thread::spawn(move || {
                    let status = child.wait();
                    println!("The desktop app closed ({status:?}).");
                    let _ = sender.send(Stop::Finished);
                });
            } else {
                println!("Press Enter (or Ctrl+C) to stop the team server.");
            }
        }
    }
    let reason = stop.recv().unwrap_or(Stop::Finished);
    server.stop();
    // The server has let go of its files, so they can be deleted.
    let removed = folder.remove();
    if reason == Stop::Interrupted {
        eprintln!("Stopped the team server and deleted its data.");
        std::process::exit(130);
    }
    removed?;
    Ok(())
}

/// Where `kicad-libraries` reads the team catalog.
#[derive(Debug, PartialEq)]
enum LibrariesFrom {
    /// A disposable team server with the sample library's parts.
    Local,
    /// A running team server, signed in as this email.
    Server { url: String, email: String },
}

/// Where `kicad-libraries` reads from and the folder it writes; `None`
/// for arguments that are not understood.
fn libraries_options(args: &[String]) -> Option<(LibrariesFrom, PathBuf)> {
    let (mut url, mut email, mut folder) = (None, None, None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--server" if url.is_none() => url = Some(rest.next()?.clone()),
            "--email" if email.is_none() => email = Some(rest.next()?.clone()),
            other if !other.starts_with("--") && folder.is_none() => {
                folder = Some(PathBuf::from(other));
            }
            _ => return None,
        }
    }
    let from = match (url, email) {
        (None, None) => LibrariesFrom::Local,
        (Some(url), Some(email)) => LibrariesFrom::Server { url, email },
        _ => return None,
    };
    Some((from, folder?))
}

/// The `--team-code CODE` among `args`, checked, and the other arguments;
/// `None` when it is given twice or is not a team code.
fn team_code_option(args: &[String]) -> Option<(Option<String>, Vec<String>)> {
    let mut code = None;
    let mut rest = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--team-code" {
            let value = args.next()?;
            if code.is_some() || kicad_libraries::team_code_problem(value).is_some() {
                return None;
            }
            code = Some(value.clone());
        } else {
            rest.push(arg.clone());
        }
    }
    Some((code, rest))
}

fn kicad_libraries(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (team_code, args) = team_code_option(args).ok_or(USAGE)?;
    let (from, folder) = libraries_options(&args).ok_or(USAGE)?;
    let folder = std::path::absolute(folder)?;
    let started = Instant::now();
    let mut local = None;
    let (client, server_url) = match from {
        LibrariesFrom::Server { url, email } => {
            if team_code.is_some() {
                return Err("The team code of a running team server is chosen in the desktop app, not with --team-code.".into());
            }
            let password = std::env::var("KICAD_PARTS_PASSWORD").map_err(
                |_| "Set KICAD_PARTS_PASSWORD to the password of the team member you sign in as.",
            )?;
            let client = collaboration::PocketBaseClient::new(&url)
                .authenticate("users", &email, &password)?;
            (client, url)
        }
        LibrariesFrom::Local => {
            let Some(code) = &team_code else {
                return Err("Give the disposable team server a team code, such as --team-code MA: it names the KiCad libraries.".into());
            };
            let root = services::repository_root();
            let run = RunFolder::create(&root.join("target").join("local-team-server"))?;
            let (server, admin, _superuser) = services::start(&root, run.path())?;
            sample_library::seed(&admin, &[])?;
            submit_team_code(&admin, code)?;
            let url = server.url().to_owned();
            local = Some((server, run));
            (admin, url)
        }
    };
    let settings = read_team(&client)?;
    let naming = kicad_libraries::Naming::new(&settings.code).map_err(|_| {
        format!(
            "The team on {server_url} has no team code yet, so no KiCad libraries are made. Someone who can edit the team catalog chooses it in the desktop app's KiCad settings."
        )
    })?;
    let team = if local.is_some() {
        libraries::disposable_team(naming)
    } else {
        kicad_libraries::Team {
            id: settings.id,
            server: server_url,
            naming,
        }
    };
    let started_up = started.elapsed();
    let reading = Instant::now();
    let readback = read_back(&client)?;
    let read = reading.elapsed();
    if let Some((server, run)) = local {
        server.stop();
        run.remove()?;
    }
    let writing = Instant::now();
    let outcome = libraries::write(&folder, &readback, &team)?;
    let written = writing.elapsed();
    let libraries: Vec<serde_json::Value> = outcome
        .libraries
        .iter()
        .map(|library| serde_json::json!({ "nickname": library.nickname, "path": library.path }))
        .collect();
    let summary = serde_json::json!({
        "folder": outcome.folder.display().to_string(),
        "team_code": team.naming.code(),
        "entries": {
            "current": outcome.current,
            "older": outcome.older,
            "archived": outcome.archived,
        },
        "files_written": outcome.written.len(),
        "database_changed": outcome.database_changed,
        "layout_changed": outcome.layout_changed,
        "libraries": libraries,
        "problems": outcome.problems,
        "seconds": {
            "start_and_seed": started_up.as_secs_f64(),
            "read_team_catalog": read.as_secs_f64(),
            "write_libraries": written.as_secs_f64(),
        },
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

/// What to do after starting; `None` for arguments that are not
/// understood.
fn server_options(args: &[String]) -> Option<Then> {
    let mut then = None;
    for arg in args {
        match arg.as_str() {
            "--desktop" | "--json" if then.is_none() => {
                then = Some(if arg == "--desktop" {
                    Then::OpenDesktop
                } else {
                    Then::PrintJson
                });
            }
            _ => return None,
        }
    }
    Some(then.unwrap_or(Then::WaitForEnter))
}

#[derive(Debug, PartialEq)]
enum Stop {
    /// Enter, the desktop closing, or the input closing.
    Finished,
    /// Ctrl+C, Ctrl+Break or the console window closing.
    Interrupted,
}

/// Where the Ctrl+C handler sends its request to stop.
static INTERRUPTS: Mutex<Option<mpsc::Sender<Stop>>> = Mutex::new(None);

/// Turns Ctrl+C, Ctrl+Break and closing the console into an orderly stop,
/// so the server is stopped and its data deleted rather than left behind.
#[cfg(windows)]
fn stop_on_ctrl_c(sender: mpsc::Sender<Stop>) {
    use windows_sys::Win32::System::Console::{CTRL_CLOSE_EVENT, SetConsoleCtrlHandler};
    use windows_sys::core::BOOL;

    unsafe extern "system" fn handler(event: u32) -> BOOL {
        if let Some(sender) = INTERRUPTS.lock().ok().and_then(|guard| guard.clone()) {
            let _ = sender.send(Stop::Interrupted);
        }
        if event == CTRL_CLOSE_EVENT {
            // Windows ends the process when this returns; give the main
            // thread time to stop the server and delete its data.
            std::thread::sleep(std::time::Duration::from_secs(4));
        }
        1
    }

    *INTERRUPTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(sender);
    // SAFETY: `handler` is a plain function that lives for the whole
    // process and only touches the mutex above.
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

#[cfg(not(windows))]
fn stop_on_ctrl_c(sender: mpsc::Sender<Stop>) {
    // Without a handler, Ctrl+C ends the process; the next run sweeps the
    // folder.
    *INTERRUPTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(sender);
}

fn describe(member: &Member) -> String {
    format!(
        "  {:<28} password {}   ({}, {})",
        member.email,
        member.password,
        member.name,
        if member.can_edit_catalog {
            "can edit"
        } else {
            "can only read"
        }
    )
}

/// The desktop app built next to this command (in the same target folder).
fn desktop_binary() -> PathBuf {
    let name = if cfg!(windows) {
        "kicad-parts-desktop.exe"
    } else {
        "kicad-parts-desktop"
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_team_server_takes_one_mode() {
        let args =
            |text: &str| -> Vec<String> { text.split_whitespace().map(str::to_owned).collect() };
        let options = |text: &str| server_options(&args(text));
        assert_eq!(options(""), Some(Then::WaitForEnter));
        assert_eq!(options("--json"), Some(Then::PrintJson));
        assert_eq!(options("--desktop"), Some(Then::OpenDesktop));
        for refused in ["--json --desktop", "--other", "g-res", "--sample"] {
            assert!(options(refused).is_none(), "{refused}");
        }
    }

    #[test]
    fn a_team_code_is_taken_out_and_checked() {
        let args =
            |text: &str| -> Vec<String> { text.split_whitespace().map(str::to_owned).collect() };
        assert_eq!(
            team_code_option(&args("--json --team-code MA --desktop")),
            Some((Some("MA".to_owned()), args("--json --desktop")))
        );
        assert_eq!(team_code_option(&args("out")), Some((None, args("out"))));
        for refused in [
            "--team-code",
            "--team-code ma",
            "--team-code MA --team-code SL",
        ] {
            assert_eq!(team_code_option(&args(refused)), None, "{refused}");
        }
    }

    #[test]
    fn kicad_libraries_reads_a_local_or_a_running_server() {
        let args =
            |text: &str| -> Vec<String> { text.split_whitespace().map(str::to_owned).collect() };
        let options = |text: &str| libraries_options(&args(text));
        assert_eq!(
            options("out"),
            Some((LibrariesFrom::Local, PathBuf::from("out")))
        );
        assert_eq!(
            options("--server http://127.0.0.1:8090 --email a@example.test out"),
            Some((
                LibrariesFrom::Server {
                    url: "http://127.0.0.1:8090".into(),
                    email: "a@example.test".into()
                },
                PathBuf::from("out")
            ))
        );
        for refused in ["", "a b", "--server http://x out", "--json out"] {
            assert_eq!(options(refused), None, "{refused}");
        }
    }
}
