use legacy_import_fixture::{build_manifest, compare, load_manifest, manifest_json};
use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("check");
    let root = PathBuf::from(args.get(1).map(String::as_str).unwrap_or("."));
    let manifest_path = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("fixtures/import/legacy-catalog/manifest.json"));
    let actual = build_manifest(&root)?;
    match command {
        "write" => {
            std::fs::write(&manifest_path, manifest_json(&actual)?)?;
            println!("wrote {}", manifest_path.display());
        }
        "check" => {
            let expected = load_manifest(&manifest_path)?;
            let differences = compare(&expected, &actual);
            if differences.is_empty() {
                println!("legacy import fixture matches {}", manifest_path.display());
            } else {
                for difference in &differences {
                    eprintln!("{difference}");
                }
                return Err(format!("{} fixture difference(s)", differences.len()).into());
            }
        }
        _ => {
            return Err(
                "usage: legacy-import-fixture [check|write] [REPOSITORY_ROOT] [MANIFEST_PATH]"
                    .into(),
            );
        }
    }
    Ok(())
}
