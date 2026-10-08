//! `catalog-import pilot [REPOSITORY_ROOT] [GROUPING]`
//!
//! Imports one Legacy Catalog grouping (default `g-res`) into a disposable,
//! loopback-only PocketBase, imports it again to show nothing new is
//! created, reads every Catalog record back, and prints the zero-loss
//! report against the oracle. The PocketBase and its data are deleted when
//! the command ends; the repository is only read. Exits 1 if anything was
//! lost.

use catalog::LegacyCatalog;
use catalog::pocketbase::{ImportCommand, ImportOutcome, read_back, submit_import};
use catalog_import::services;
use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("pilot") {
        return Err("usage: catalog-import pilot [REPOSITORY_ROOT] [GROUPING]".into());
    }
    let root = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(services::repository_root);
    let grouping = args.get(2).map(String::as_str).unwrap_or("g-res");
    let oracle = legacy_import_fixture::load_manifest(
        &root.join("fixtures/import/legacy-catalog/manifest.json"),
    )?;

    let commit = services::source_commit(&root);
    let import = LegacyCatalog::new(&root).import(grouping, commit)?;
    let reason = format!(
        "Import the Legacy Catalog grouping {grouping} (database/{grouping}.csv) into canonical Part Revisions"
    );

    let state = tempfile::tempdir()?;
    let (server, admin, _superuser) = services::start(&services::repository_root(), state.path())?;
    let first = submit_import(&admin, &ImportCommand::new(&import, &reason))?;
    let after_first = read_back(&admin)?;
    let mut retry = ImportCommand::new(&import, &reason);
    retry.correlation_id.push_str("-retry");
    let second = submit_import(&admin, &retry)?;
    let readback = read_back(&admin)?;
    server.stop();

    let report = catalog_import::report(&oracle, grouping, &readback, &root);
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
