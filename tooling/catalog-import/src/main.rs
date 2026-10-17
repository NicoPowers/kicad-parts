//! `catalog-import pilot [--allow-uncommitted-changes] [REPOSITORY_ROOT] [CSV_FILE]`
//!
//! Imports one CSV catalog file (default `g-res`) into a disposable,
//! loopback-only PocketBase, imports it again to show nothing new is
//! created, reads every Catalog record back, and prints the zero-loss
//! report against the oracle. Each import does what an import into the
//! team catalog does: it refuses files with changes that are not committed
//! unless `--allow-uncommitted-changes` is given, reads the team catalog,
//! refuses an older checkout, runs the library check (printed to standard
//! error; an error in a file the import uses stops it before anything is
//! sent) and expects each part's current revision. The PocketBase and its
//! data are deleted when the command ends; the repository is only read.
//! Exits 1 if anything was lost.
//!
//! `catalog-import full-pilot [--allow-uncommitted-changes] [--every-difference] [REPOSITORY_ROOT]`
//!
//! The full-catalog proof (`full_proof`): imports every CSV file into one
//! throwaway team server as a team would, twice; checks nothing is lost,
//! file by file and across files; writes the KiCad libraries, again from
//! the same server and from a second one, and shows they are
//! byte-identical; compares them with `database/parts.sqlite`; and shows
//! the CSV catalog did not change. Prints a report for people;
//! `--every-difference` lists each difference from `parts.sqlite` on its
//! own line. Everything it writes is in `target/full-catalog-proof/` and is
//! deleted when it ends. Exits 1 if anything was lost or differs without a
//! reason.
//!
//! `catalog-import local-team-server [--desktop | --json] [--csv FILES] [--team-code CODE]`
//!
//! Starts a disposable team server on 127.0.0.1 for trying the desktop app
//! by hand: a PocketBase with every CSV file of the CSV catalog imported
//! (`database/g-*.csv`, one category each) and three team members (two who
//! can edit, one who can only read), each with a new random password. It
//! prints the server address and the accounts, then:
//!
//! - with `--team-code MA`, gives the team that code, which names its KiCad
//!   libraries (MA-Parts); without it, someone who can edit chooses it in
//!   the desktop app;
//! - with `--csv g-res,g-cap` (a comma list; may be given more than once),
//!   imports only those CSV files, which starts faster and leaves the rest
//!   of the CSV catalog's parts out of the team catalog;
//!
//! - with `--desktop`, opens the built desktop app pointed at the server
//!   and stops the server when the app closes;
//! - with `--json`, prints one JSON line instead of text and stops when
//!   standard input closes (the desktop journeys use this). Each line
//!   `reimport` it reads imports the resistors again, as an importer would
//!   after reading the team catalog, and prints one JSON line with the
//!   outcome (`{"reimported":"nothing_changed"}`); `reimport <folder>`
//!   imports them from a copy of the CSV catalog in that folder instead.
//!   Each reimport runs the library check like a real import, so a copy
//!   with a new footprint whose 3D model link is outside the library is
//!   refused (`{"reimported":"refused: The library check found ..."}`);
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
//!
//! `catalog-import kicad-libraries --team-code CODE [--csv FILES] FOLDER`
//! `catalog-import kicad-libraries --server URL --email EMAIL FOLDER`
//!
//! Writes the KiCad libraries the desktop app makes (kicad-parts-f397) into
//! `FOLDER/<team code>`, without the app: the database library with one
//! entry per part revision, and the symbols, footprints and 3D models it
//! uses. With `--server`, it reads that team server, signed in as `EMAIL`
//! with the password in the `KICAD_PARTS_PASSWORD` environment variable,
//! and refuses if the team has no code yet. Otherwise it starts a
//! disposable team server with every CSV file imported (or only the
//! `--csv` ones) and the code `--team-code` gives, as `local-team-server`
//! does, and deletes it after reading. Prints one JSON summary: entry
//! counts, files written, problems, and how long reading and writing took.
//! Libraries from disposable team servers count as one team's, so running
//! it again updates the same folder.

use catalog::library_check::Group;
use catalog::pocketbase::{
    ImportCommand, ImportOutcome, read_back, read_import_state, read_team, submit_import,
    submit_team_code,
};
use catalog_import::run_folder::RunFolder;
use catalog_import::services::{self, Allow, Member};
use catalog_import::{full_proof, imports, libraries};
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

const USAGE: &str = "usage: catalog-import pilot [--allow-uncommitted-changes] [REPOSITORY_ROOT] [CSV_FILE]\n       catalog-import full-pilot [--allow-uncommitted-changes] [--every-difference] [REPOSITORY_ROOT]\n       catalog-import local-team-server [--desktop | --json] [--csv g-res,g-cap ...] [--team-code MA]\n       catalog-import kicad-libraries --team-code MA [--csv g-res,g-cap ...] FOLDER\n       catalog-import kicad-libraries --server URL --email EMAIL FOLDER";

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("pilot") => pilot(&args[1..]),
        Some("full-pilot") => full_pilot(&args[1..]),
        Some("local-team-server") => local_team_server(&args[1..]),
        Some("kicad-libraries") => kicad_libraries(&args[1..]),
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
    // Refuse uncommitted changes before starting PocketBase; each import
    // checks again.
    services::check_committed(&root, &import, allow)?;

    let state = tempfile::tempdir()?;
    let (server, admin, _superuser) = services::start(&services::repository_root(), state.path())?;
    let imported = imports::import_twice(&admin, &root, std::slice::from_ref(&import), allow);
    server.stop();
    let imported = imported?;
    let (first, second) = (&imported.first[0], &imported.second[0]);
    // Standard output is the JSON summary; the check is for the person.
    eprintln!("{}", first.check.report());

    let report = catalog_import::report(&oracle, csv_file, &imported.readback, &root);
    let mut summary = report.summary();
    let check = &first.check;
    summary["library_check"] = serde_json::json!({
        "to_fix": check.count(Group::ToFix),
        "not_in_catalog": check.count(Group::NotInCatalog),
        "worth_a_look": check.count(Group::WorthALook),
        "clean_up": check.count(Group::CleanUp),
    });
    summary["import"] = serde_json::json!({
        "first": first.reply_json(),
        "second": second.reply_json(),
        "second_created_nothing": imported.second_created_nothing,
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    if !report.is_lossless() {
        return Err(format!("{} problem(s)", report.problems.len()).into());
    }
    Ok(())
}

fn full_pilot(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    const ALLOW_UNCOMMITTED: &str = "--allow-uncommitted-changes";
    const EVERY_DIFFERENCE: &str = "--every-difference";
    let allow = Allow {
        uncommitted_changes: args.iter().any(|arg| arg == ALLOW_UNCOMMITTED),
        ..Allow::default()
    };
    let every_difference = args.iter().any(|arg| arg == EVERY_DIFFERENCE);
    let positional: Vec<&String> = args
        .iter()
        .filter(|arg| *arg != ALLOW_UNCOMMITTED && *arg != EVERY_DIFFERENCE)
        .collect();
    if positional.len() > 1 || positional.iter().any(|arg| arg.starts_with("--")) {
        return Err(USAGE.into());
    }
    let root = positional
        .first()
        .map(|arg| PathBuf::from(arg.as_str()))
        .unwrap_or_else(services::repository_root);
    let oracle = csv_catalog_fixture::load_manifest(
        &root.join("fixtures/import/csv-catalog/manifest.json"),
    )?;
    // Under target/, never the system temp folder; a run that is killed
    // outright is deleted by the next one.
    let work = RunFolder::create(&full_proof::work_base(&services::repository_root()))?;
    let proof = full_proof::run(&root, &oracle, work.path(), allow);
    work.remove()?;
    let proof = proof?;
    print!("{}", full_proof::report::text(&proof, every_difference));
    if !proof.passed() {
        return Err(format!(
            "{} thing(s) lost or different without a reason",
            proof.failures().len()
        )
        .into());
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
    let (team_code, args) = team_code_option(args).ok_or(USAGE)?;
    let (then, chosen) = server_options(&args).ok_or(USAGE)?;
    let desktop = desktop_binary();
    if then == Then::OpenDesktop && !desktop.is_file() {
        return Err(format!(
            "{} is not built yet. Build it first:\n  trunk build              (in apps/desktop-ui)\n  cargo build --locked -p kicad-parts-desktop --features custom-protocol",
            desktop.display()
        )
        .into());
    }

    let root = services::repository_root();
    let every = services::csv_files(&root)?;
    let all = chosen.is_empty();
    let csv_files = if all {
        every
    } else {
        if let Some(unknown) = chosen.iter().find(|name| !every.contains(name)) {
            return Err(
                format!("database/{unknown}.csv is not a CSV file of the CSV catalog").into(),
            );
        }
        chosen
    };
    let planned = services::plan_all(&root, &csv_files)?;
    // `reimport` imports the resistors again.
    let import = match planned.iter().find(|import| import.csv_file == "g-res") {
        Some(import) => import.clone(),
        None => services::plan(&root, "g-res")?,
    };
    // A known folder under target/, not the system temp folder, so a run
    // that is killed outright is cleaned up by the next one.
    let folder = RunFolder::create(&root.join("target").join("local-team-server"))?;
    let (stop_sender, stop) = mpsc::channel::<Stop>();
    stop_on_ctrl_c(stop_sender.clone());
    let (server, admin, _superuser) = services::start(&root, folder.path())?;
    // Every CSV file (unless --csv names some), so every category exists.
    // Files share CAD items (one
    // footprint serves several categories), so each import reads the team
    // catalog first, as an importer does.
    import_all(&admin, &planned)?;
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
            if all {
                writeln!(
                    out,
                    "All {} CSV files in database/ are imported, one category each.",
                    csv_files.len()
                )?;
            } else {
                writeln!(
                    out,
                    "These CSV files are imported, one category each: {}.",
                    csv_files.join(", ")
                )?;
            }
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
                            Ok(outcome) => match outcome.strip_prefix("refused: ") {
                                Some(why) => println!("The import was refused. {why}"),
                                None => println!("Imported again: {}.", outcome.replace('_', " ")),
                            },
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

/// Imports `planned` one CSV file after another, each reading the team
/// catalog first, as an importer does.
fn import_all(
    admin: &collaboration::PocketBaseClient,
    planned: &[catalog::CsvImport],
) -> Result<(), Box<dyn std::error::Error>> {
    for each in planned {
        let csv_file = &each.csv_file;
        let state = read_import_state(admin)?;
        let command = ImportCommand::new(
            each,
            &format!("Import the parts from database/{csv_file}.csv"),
        )
        .expecting(&state)
        .allowing_uncommitted_changes(services::has_uncommitted_changes(each));
        let reply = submit_import(admin, &command)?;
        if !matches!(reply.outcome, ImportOutcome::Applied { .. }) {
            return Err(format!("the import of {csv_file} did not apply: {}", reply.body).into());
        }
    }
    Ok(())
}

/// Where `kicad-libraries` reads the team catalog.
#[derive(Debug, PartialEq)]
enum LibrariesFrom {
    /// A disposable team server with these CSV files (empty for all).
    Local(Vec<String>),
    /// A running team server, signed in as this email.
    Server { url: String, email: String },
}

/// Where `kicad-libraries` reads from and the folder it writes; `None`
/// for arguments that are not understood.
fn libraries_options(args: &[String]) -> Option<(LibrariesFrom, PathBuf)> {
    let mut chosen = Vec::new();
    let (mut url, mut email, mut folder) = (None, None, None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--csv" => {
                for name in rest.next()?.split(',').map(str::trim) {
                    if name.is_empty() {
                        return None;
                    }
                    let name = name.trim_end_matches(".csv").to_owned();
                    if !chosen.contains(&name) {
                        chosen.push(name);
                    }
                }
            }
            "--server" if url.is_none() => url = Some(rest.next()?.clone()),
            "--email" if email.is_none() => email = Some(rest.next()?.clone()),
            other if !other.starts_with("--") && folder.is_none() => {
                folder = Some(PathBuf::from(other));
            }
            _ => return None,
        }
    }
    let from = match (url, email) {
        (None, None) => LibrariesFrom::Local(chosen),
        (Some(url), Some(email)) if chosen.is_empty() => LibrariesFrom::Server { url, email },
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
        LibrariesFrom::Local(chosen) => {
            let Some(code) = &team_code else {
                return Err("Give the disposable team server a team code, such as --team-code MA: it names the KiCad libraries.".into());
            };
            let root = services::repository_root();
            let csv_files = if chosen.is_empty() {
                services::csv_files(&root)?
            } else {
                chosen
            };
            let planned = services::plan_all(&root, &csv_files)?;
            let run = RunFolder::create(&root.join("target").join("local-team-server"))?;
            let (server, admin, _superuser) = services::start(&root, run.path())?;
            import_all(&admin, &planned)?;
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
    let imported = started.elapsed();
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
            "start_and_import": imported.as_secs_f64(),
            "read_team_catalog": read.as_secs_f64(),
            "write_libraries": written.as_secs_f64(),
        },
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

/// What to do after starting, and the CSV files `--csv` names (empty for
/// every file); `None` for arguments that are not understood.
fn server_options(args: &[String]) -> Option<(Then, Vec<String>)> {
    let mut then = None;
    let mut chosen: Vec<String> = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--desktop" | "--json" if then.is_none() => {
                then = Some(if arg == "--desktop" {
                    Then::OpenDesktop
                } else {
                    Then::PrintJson
                });
            }
            "--csv" => {
                let list = rest.next()?;
                for name in list.split(',').map(str::trim) {
                    if name.is_empty() {
                        return None;
                    }
                    let name = name.trim_end_matches(".csv").to_owned();
                    if !chosen.contains(&name) {
                        chosen.push(name);
                    }
                }
            }
            _ => return None,
        }
    }
    Some((then.unwrap_or(Then::WaitForEnter), chosen))
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
        None => reimport(admin, import, &services::repository_root()),
        Some(folder) => reimport(admin, &services::plan(&folder, "g-res")?, &folder),
    }
}

/// Imports the resistors again as an importer would after reading the team
/// catalog, and names the outcome. Uncommitted changes are allowed, because
/// this team server is thrown away when it stops; the library check runs as
/// for any import, so an error in a file it uses refuses it.
fn reimport(
    admin: &collaboration::PocketBaseClient,
    import: &catalog::CsvImport,
    folder: &std::path::Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let state = read_import_state(admin)?;
    if let Err(refusal) = services::check_libraries(folder, import, &state) {
        return Ok(format!("refused: {refusal}"));
    }
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
    fn the_team_server_takes_a_mode_and_csv_files() {
        let args =
            |text: &str| -> Vec<String> { text.split_whitespace().map(str::to_owned).collect() };
        let options = |text: &str| server_options(&args(text));
        assert!(
            options("")
                .is_some_and(|(then, chosen)| then == Then::WaitForEnter && chosen.is_empty())
        );
        let (then, chosen) = options("--json --csv g-res,g-cap --csv g-reg.csv,g-res").unwrap();
        assert!(then == Then::PrintJson);
        assert_eq!(chosen, ["g-res", "g-cap", "g-reg"]);
        assert!(
            options("--csv g-res --desktop").is_some_and(|(then, _)| then == Then::OpenDesktop)
        );
        for refused in [
            "--csv",
            "--csv g-res,,g-cap",
            "--json --desktop",
            "--other",
            "g-res",
        ] {
            assert!(options(refused).is_none(), "{refused}");
        }
    }

    #[test]
    fn a_team_code_is_taken_out_and_checked() {
        let args =
            |text: &str| -> Vec<String> { text.split_whitespace().map(str::to_owned).collect() };
        assert_eq!(
            team_code_option(&args("--json --team-code MA --csv g-res")),
            Some((Some("MA".to_owned()), args("--json --csv g-res")))
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
            Some((LibrariesFrom::Local(Vec::new()), PathBuf::from("out")))
        );
        assert_eq!(
            options("--csv g-res,g-cap.csv out"),
            Some((
                LibrariesFrom::Local(vec!["g-res".into(), "g-cap".into()]),
                PathBuf::from("out")
            ))
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
        for refused in [
            "",
            "--csv g-res",
            "a b",
            "--server http://x out",
            "--server http://x --email e --csv g-res out",
            "--json out",
        ] {
            assert_eq!(options(refused), None, "{refused}");
        }
    }

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
