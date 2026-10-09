//! `catalog-import pilot [REPOSITORY_ROOT] [CSV_FILE]`
//!
//! Imports one CSV catalog file (default `g-res`) into a disposable,
//! loopback-only PocketBase, imports it again to show nothing new is
//! created, reads every Catalog record back, and prints the zero-loss
//! report against the oracle. The PocketBase and its data are deleted when
//! the command ends; the repository is only read. Exits 1 if anything was
//! lost.
//!
//! `catalog-import local-team-server [--desktop | --json]`
//!
//! Starts a disposable team server on 127.0.0.1 for trying the desktop app
//! by hand: a PocketBase with the resistors imported and three team
//! members (two who can edit, one who can only read), each with a new
//! random password. It prints the server address and the accounts, then:
//!
//! - with `--desktop`, opens the built desktop app pointed at the server
//!   and stops the server when the app closes;
//! - with `--json`, prints one JSON line instead of text and stops when
//!   standard input closes (the desktop journeys use this);
//! - otherwise, stops when you press Enter.
//!
//! The server runs without a window in a kill-on-close job, so it also
//! stops if this command is killed. Nothing is written to the repository.

use catalog::pocketbase::{ImportCommand, ImportOutcome, read_back, submit_import};
use catalog_import::services::{self, Member};
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::Command;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

const USAGE: &str = "usage: catalog-import pilot [REPOSITORY_ROOT] [CSV_FILE]\n       catalog-import local-team-server [--desktop | --json]";

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("pilot") => pilot(&args[1..]),
        Some("local-team-server") => local_team_server(&args[1..]),
        _ => Err(USAGE.into()),
    }
}

fn pilot(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let root = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(services::repository_root);
    let csv_file = args.get(1).map(String::as_str).unwrap_or("g-res");
    let oracle = legacy_import_fixture::load_manifest(
        &root.join("fixtures/import/legacy-catalog/manifest.json"),
    )?;

    let import = services::plan(&root, csv_file)?;
    let reason = format!("Import the parts from database/{csv_file}.csv");

    let state = tempfile::tempdir()?;
    let (server, admin, _superuser) = services::start(&services::repository_root(), state.path())?;
    let first = submit_import(&admin, &ImportCommand::new(&import, &reason))?;
    let after_first = read_back(&admin)?;
    let mut retry = ImportCommand::new(&import, &reason);
    retry.correlation_id.push_str("-retry");
    let second = submit_import(&admin, &retry)?;
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
    let import = services::plan(&root, "g-res")?;
    let state = tempfile::tempdir()?;
    let (server, admin, _superuser) = services::start(&root, state.path())?;
    let reply = submit_import(
        &admin,
        &ImportCommand::new(&import, "Import the resistors from database/g-res.csv"),
    )?;
    if !matches!(reply.outcome, ImportOutcome::Applied { .. }) {
        return Err(format!("the import did not apply: {}", reply.body).into());
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
            // Stop when whoever started us closes our input.
            let mut sink = Vec::new();
            let _ = std::io::stdin().lock().read_to_end(&mut sink);
        }
        Then::WaitForEnter | Then::OpenDesktop => {
            writeln!(out, "Team server: {}", server.url())?;
            writeln!(out, "The resistors from database/g-res.csv are imported.")?;
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
            if then == Then::OpenDesktop {
                println!("Opening the desktop app. Close it to stop the team server.");
                let status = Command::new(&desktop)
                    .arg(format!("--team-server={}", server.url()))
                    .arg(format!("--team-email={}", members[0].email))
                    .arg(format!(
                        "--data-dir={}",
                        state.path().join("desktop").display()
                    ))
                    .status()?;
                println!("The desktop app closed ({status}).");
            } else {
                println!("Press Enter to stop the team server.");
                let mut line = String::new();
                let _ = std::io::stdin().lock().read_line(&mut line);
            }
        }
    }
    server.stop();
    Ok(())
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
