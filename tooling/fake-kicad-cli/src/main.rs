//! A stand-in kicad-cli for tests of version handling. Copy it next to a
//! `fake-version.txt` and name the copy `kicad-cli(.exe)`: `version` prints
//! the file's contents. Without the file it fails like a broken install.

use std::process::ExitCode;

fn main() -> ExitCode {
    let version_file = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("fake-version.txt")));
    let arg = std::env::args().nth(1).unwrap_or_default();
    match (
        arg.as_str(),
        version_file.and_then(|f| std::fs::read_to_string(f).ok()),
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
