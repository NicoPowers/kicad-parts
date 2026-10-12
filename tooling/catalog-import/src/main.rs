//! `catalog-import pilot [--allow-uncommitted-changes] [REPOSITORY_ROOT] [CSV_FILE]`
//!
//! Imports one CSV catalog file (default `g-res`) into a disposable,
//! loopback-only PocketBase, imports it again to show nothing new is
//! created, reads every Catalog record back, and prints the zero-loss
//! report against the oracle. Each import does what an import into the
//! team catalog does: it refuses files with changes that are not committed
//! unless `--allow-uncommitted-changes` is given, reads the team catalog,
//! refuses an older checkout and expects each part's current revision. The
//! PocketBase and its data are deleted when the command ends; the
//! repository is only read. Exits 1 if anything was lost.
//!
//! `catalog-import local-team-server [--desktop | --json]`
//!
//! Starts a disposable team server on 127.0.0.1 for trying the desktop app
//! by hand: a PocketBase with every CSV file of the CSV catalog imported
//! (`database/g-*.csv`, one category each) and three team members (two who
//! can edit, one who can only read), each with a new random password. It
//! prints the server address and the accounts, then:
//!
//! - with `--desktop`, opens the built desktop app pointed at the server
//!   and stops the server when the app closes;
//! - with `--json`, prints one JSON line instead of text and stops when
//!   standard input closes (the desktop journeys use this). Each line
//!   `reimport` it reads imports the resistors again, as an importer would
//!   after reading the team catalog, and prints one JSON line with the
//!   outcome (`{"reimported":"nothing_changed"}`); `reimport <folder>`
//!   imports them from a copy of the CSV catalog in that folder instead;
//! - otherwise, stops when you press Enter.
//!
//! The team server is thrown away when it stops, so its imports are allowed
//! from files with uncommitted changes (each such run records that), and
//! trying out a change does not need a commit.
//!
//! Without `--json`, typing `reimport` (or `reimport <folder>`, a copy of
//! the CSV catalog) and pressing Enter imports the resistors again, to see
//! what an import does to parts edited in the app, and prints the outcome in
//! words. Other text gets a short reminder; Enter alone stops the server.
//!
//! The server runs without a window in a kill-on-close job, so it also
//! stops if this command is killed. Its data lives in
//! `target/local-team-server/run-<process id>` and is deleted when it stops,
//! including on Ctrl+C; a run killed outright is deleted by the next run.
//! Nothing else is written.

use catalog::pocketbase::{
    ImportCommand, ImportOutcome, read_back, read_import_state, submit_import,
};
use catalog_import::run_folder::RunFolder;
use catalog_import::services::{self, Allow, Member};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, mpsc};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

const USAGE: &str = "usage: catalog-import pilot [--allow-uncommitted-changes] [REPOSITORY_ROOT] [CSV_FILE]\n       catalog-import local-team-server [--desktop | --json]";

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("pilot") => pilot(&args[1..]),
        Some("local-team-server") => local_team_server(&args[1..]),
        _ => Err(USAGE.into()),
    }
}

fn pilot(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    const ALLOW_UNCOMMITTED: &str = "--allow-uncommitted-changes";
    let allow = Allow {
        uncommitted_changes: args.iter().any(|arg| arg == ALLOW_UNCOMMITTED),
        ..Allow::default()
    };
    let positional: Vec<&String> = args
        .iter()
        .filter(|arg| *arg != ALLOW_UNCOMMITTED)
        .collect();
    if positional.len() > 2 || positional.iter().any(|arg| arg.starts_with("--")) {
        return Err(USAGE.into());
    }
    let root = positional
        .first()
        .map(|arg| PathBuf::from(arg.as_str()))
        .unwrap_or_else(services::repository_root);
    let csv_file = positional.get(1).map(|arg| arg.as_str()).unwrap_or("g-res");
    let oracle = csv_catalog_fixture::load_manifest(
        &root.join("fixtures/import/csv-catalog/manifest.json"),
    )?;

    let import = services::plan(&root, csv_file)?;
    // Refuse uncommitted changes before starting PocketBase; prepare_import
    // checks again.
    services::check_committed(&root, &import, allow)?;
    let reason = format!("Import the parts from database/{csv_file}.csv");

    let state = tempfile::tempdir()?;
    let (server, admin, _superuser) = services::start(&services::repository_root(), state.path())?;
    // Each import reads the team catalog first, as a real import does: it
    // refuses an older checkout and expects each part's current revision.
    let first_command = services::prepare_import(&admin, &root, &import, &reason, allow)?;
    let first = submit_import(&admin, &first_command)?;
    let after_first = read_back(&admin)?;
    // Its correlation ID differs from the first one's because it expects
    // the parts the first one made.
    let second_command = services::prepare_import(&admin, &root, &import, &reason, allow)?;
    let second = submit_import(&admin, &second_command)?;
    let readback = read_back(&admin)?;
    server.stop();

    let report = catalog_import::report(&oracle, csv_file, &readback, &root);
    let mut summary = report.summary();
    summary["import"] = serde_json::json!({
        "first": serde_json::from_str::<serde_json::Value>(&first.body)?,
        "second": serde_json::from_str::<serde_json::Value>(&second.body)?,
        "second_created_nothing": matches!(second.outcome, ImportOutcome::Noop { .. })
            && after_first == readback,
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    if !report.is_lossless() {
        return Err(format!("{} problem(s)", report.problems.len()).into());
    }
    Ok(())
}

#[derive(PartialEq)]
enum Then {
    WaitForEnter,
    OpenDesktop,
    PrintJson,
}

fn local_team_server(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let then = match args {
        [] => Then::WaitForEnter,
        [flag] if flag == "--desktop" => Then::OpenDesktop,
        [flag] if flag == "--json" => Then::PrintJson,
        _ => return Err(USAGE.into()),
    };
    let desktop = desktop_binary();
    if then == Then::OpenDesktop && !desktop.is_file() {
        return Err(format!(
            "{} is not built yet. Build it first:\n  trunk build              (in apps/desktop-ui)\n  cargo build --locked -p kicad-parts-desktop --features custom-protocol",
            desktop.display()
        )
        .into());
    }

    let root = services::repository_root();
    let csv_files = services::csv_files(&root)?;
    let planned = services::plan_all(&root, &csv_files)?;
    // `reimport` imports the resistors again.
    let import = planned
        .iter()
        .find(|import| import.csv_file == "g-res")
        .ok_or("database/g-res.csv is missing")?
        .clone();
    // A known folder under target/, not the system temp folder, so a run
    // that is killed outright is cleaned up by the next one.
    let folder = RunFolder::create(&root.join("target").join("local-team-server"))?;
    let (stop_sender, stop) = mpsc::channel::<Stop>();
    stop_on_ctrl_c(stop_sender.clone());
    let (server, admin, _superuser) = services::start(&root, folder.path())?;
    // Every CSV file, so every category exists. Files share CAD items (one
    // footprint serves several categories), so each import reads the team
    // catalog first, as an importer does.
    for each in &planned {
        let csv_file = &each.csv_file;
        let state = read_import_state(&admin)?;
        let command = ImportCommand::new(
            each,
            &format!("Import the parts from database/{csv_file}.csv"),
        )
        .expecting(&state)
        .allowing_uncommitted_changes(services::has_uncommitted_changes(each));
        let reply = submit_import(&admin, &command)?;
        if !matches!(reply.outcome, ImportOutcome::Applied { .. }) {
            return Err(format!("the import of {csv_file} did not apply: {}", reply.body).into());
        }
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
                serde_json::json!({ "url": server.url(), "accounts": accounts })
            )?;
            out.flush()?;
            drop(out);
            // Import again on each `reimport` line; stop when whoever
            // started us closes our input.
            let sender = stop_sender.clone();
            let (admin, import) = (admin.clone(), import.clone());
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines() {
                    let Ok(line) = line else { break };
                    let Some(from) = reimport_command(&line) else {
                        continue;
                    };
                    let outcome = reimport_from(&admin, &import, from)
                        .unwrap_or_else(|error| format!("failed: {error}"));
                    println!("{}", serde_json::json!({ "reimported": outcome }));
                }
                let _ = sender.send(Stop::Finished);
            });
        }
        Then::WaitForEnter | Then::OpenDesktop => {
            writeln!(out, "Team server: {}", server.url())?;
            writeln!(
                out,
                "All {} CSV files in database/ are imported, one category each.",
                csv_files.len()
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
            println!(
                "Type reimport and press Enter to import the resistors again. Type reimport <folder> to import them from a copy of the CSV catalog in that folder."
            );
            let (admin, import) = (admin.clone(), import.clone());
            let typed = (then == Then::WaitForEnter).then(|| sender.clone());
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines() {
                    let Ok(line) = line else { break };
                    if let Some(from) = reimport_command(&line) {
                        match reimport_from(&admin, &import, from) {
                            Ok(outcome) => {
                                println!("Imported again: {}.", outcome.replace('_', " "))
                            }
                            Err(error) => println!("The import failed: {error}"),
                        }
                    } else if line.trim().is_empty() {
                        // Enter alone stops the server (not with --desktop,
                        // where closing the app does).
                        if let Some(sender) = &typed {
                            let _ = sender.send(Stop::Finished);
                            return;
                        }
                    } else if typed.is_some() {
                        println!(
                            "Unknown command. Type reimport or reimport <folder>, or press Enter to stop the team server."
                        );
                    } else {
                        println!("Unknown command. Type reimport or reimport <folder>.");
                    }
                }
                if let Some(sender) = &typed {
                    let _ = sender.send(Stop::Finished);
                }
            });
            if then == Then::OpenDesktop {
                println!("Opening the desktop app. Close it to stop the team server.");
                let mut child = Command::new(&desktop)
                    .arg(format!("--team-server={}", server.url()))
                    .arg(format!("--team-email={}", members[0].email))
                    .arg(format!(
                        "--data-dir={}",
                        folder.path().join("desktop").display()
                    ))
                    .spawn()?;
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

/// For a `reimport` or `reimport <folder>` line, the folder to import from
/// (`None` for the repository); `None` for any other line.
fn reimport_command(line: &str) -> Option<Option<PathBuf>> {
    match line.trim().strip_prefix("reimport")? {
        "" => Some(None),
        folder if folder.starts_with(' ') => Some(Some(PathBuf::from(folder.trim()))),
        _ => None,
    }
}

/// Imports the resistors again, from `folder` (a copy of the CSV catalog)
/// or else as planned from the repository.
fn reimport_from(
    admin: &collaboration::PocketBaseClient,
    import: &catalog::CsvImport,
    folder: Option<PathBuf>,
) -> Result<String, Box<dyn std::error::Error>> {
    match folder {
        None => reimport(admin, import),
        Some(folder) => reimport(admin, &services::plan(&folder, "g-res")?),
    }
}

/// Imports the resistors again as an importer would after reading the team
/// catalog, and names the outcome. Uncommitted changes are allowed, because
/// this team server is thrown away when it stops.
fn reimport(
    admin: &collaboration::PocketBaseClient,
    import: &catalog::CsvImport,
) -> Result<String, Box<dyn std::error::Error>> {
    let state = read_import_state(admin)?;
    let command = ImportCommand::new(import, "Import the resistors from database/g-res.csv")
        .expecting(&state)
        .allowing_uncommitted_changes(services::has_uncommitted_changes(import));
    let reply = submit_import(admin, &command)?;
    Ok(match reply.outcome {
        ImportOutcome::Applied { .. } => "applied".to_owned(),
        ImportOutcome::NothingChanged { .. } => "nothing_changed".to_owned(),
        ImportOutcome::Noop { .. } => "noop".to_owned(),
        _ => format!("refused: {}", reply.body),
    })
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
    fn only_reimport_lines_are_commands() {
        assert_eq!(reimport_command("reimport"), Some(None));
        assert_eq!(reimport_command("  reimport  "), Some(None));
        assert_eq!(
            reimport_command("reimport C:/copy of catalog"),
            Some(Some(PathBuf::from("C:/copy of catalog")))
        );
        for other in ["", "reimprot", "reimports", "stop"] {
            assert_eq!(reimport_command(other), None, "{other:?}");
        }
    }
}
