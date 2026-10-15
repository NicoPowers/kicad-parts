//! A stand-in kicad-cli for tests of version handling. Copy it next to a
//! `fake-version.txt` and name the copy `kicad-cli(.exe)`: `version` prints
//! the file's contents. Without the file it fails like a broken install.
//!
//! Every run adds a line to `fake-runs.txt` next to the copy: its first
//! argument, then the `KICAD_CONFIG_HOME` and `KICAD_CACHE_HOME` it got,
//! separated by tabs. Tests read it to see where kicad-cli would keep its
//! settings.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let folder = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.to_path_buf()));
    let arg = std::env::args().nth(1).unwrap_or_default();
    if let Some(folder) = &folder {
        record_run(folder, &arg);
    }
    match (
        arg.as_str(),
        folder.and_then(|f| std::fs::read_to_string(f.join("fake-version.txt")).ok()),
    ) {
        ("version", Some(version)) => {
            println!("{}", version.trim());
            ExitCode::SUCCESS
        }
        ("version", None) => {
            eprintln!("fake kicad-cli: this installation is damaged");
            ExitCode::from(3)
        }
        _ => {
            eprintln!("fake kicad-cli only answers `version`");
            ExitCode::from(2)
        }
    }
}

fn record_run(folder: &Path, arg: &str) {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let line = format!(
        "{arg}\t{}\t{}\n",
        var("KICAD_CONFIG_HOME"),
        var("KICAD_CACHE_HOME")
    );
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(folder.join("fake-runs.txt"))
    {
        let _ = file.write_all(line.as_bytes());
    }
}
