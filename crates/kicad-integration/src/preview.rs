//! SVG previews exported by the installed kicad-cli, cached on disk.
//!
//! Cache layout, under an application cache directory:
//!
//! ```text
//! <root>/v1/kicad-<version>/<profile>/<sym|fp>-<library sha256>/<item>/*.svg
//! ```
//!
//! The library content hash and KiCad version are part of every path, so an
//! edited library or a different KiCad never serves a stale image, and a
//! repeat view never re-runs kicad-cli. Exports are written to a staging
//! directory and renamed into place, so a crash cannot leave a partial entry.

use crate::discovery::KicadInstall;
use crate::process::{CliOutput, run_kicad_cli};
use crate::resolver::{ResolvedFootprint, ResolvedSymbol};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

const CACHE_LAYOUT: &str = "v1";
/// KiCad's built-in colour theme, so exports do not depend on the user's
/// editor settings.
const THEME: &str = "_builtin_default";
const PROFILE: &str = "builtin-default";
const EXPORT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheOutcome {
    /// Served from the cache; kicad-cli did not run.
    Hit,
    /// kicad-cli exported the preview just now.
    Exported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewImage {
    /// Symbol unit number (1-based); `None` for footprints.
    pub unit: Option<u32>,
    pub file: PathBuf,
    pub svg: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Preview {
    /// One image per symbol unit, in unit order, or a single footprint image.
    pub images: Vec<PreviewImage>,
    pub library_sha256: String,
    pub cache: CacheOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreviewError {
    /// kicad-cli ran but did not produce the preview.
    ExportFailed {
        command: String,
        cli_message: String,
    },
    Io(String),
}

impl fmt::Display for PreviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExportFailed {
                command,
                cli_message,
            } => write!(f, "`{command}` failed: {cli_message}"),
            Self::Io(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for PreviewError {}

fn io_error(context: &str, path: &Path, error: io::Error) -> PreviewError {
    PreviewError::Io(format!("{context} {}: {error}", path.display()))
}

/// A cheap stand-in for library content: names, sizes and modification
/// times. The content hash is recomputed only when this changes.
type Fingerprint = Vec<(OsString, u64, Option<SystemTime>)>;

pub struct PreviewCache {
    root: PathBuf,
    hashes: Mutex<HashMap<PathBuf, (Fingerprint, String)>>,
    exports: AtomicUsize,
    staging_counter: AtomicU64,
}

impl PreviewCache {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            hashes: Mutex::new(HashMap::new()),
            exports: AtomicUsize::new(0),
            staging_counter: AtomicU64::new(0),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// How many times this cache has run kicad-cli to export.
    pub fn export_count(&self) -> usize {
        self.exports.load(Ordering::SeqCst)
    }

    /// Every unit of `symbol`, exported with `kicad-cli sym export svg`.
    pub fn symbol(
        &self,
        kicad: &KicadInstall,
        symbol: &ResolvedSymbol,
    ) -> Result<Preview, PreviewError> {
        let library_sha256 = self.library_hash(&symbol.library)?;
        let entry = self.entry_dir(kicad, "sym", &library_sha256, &symbol.lib_ref.item);
        self.cached_or_export(entry, library_sha256, |staging| {
            let out = staging.join("out");
            let args: Vec<OsString> = vec![
                "sym".into(),
                "export".into(),
                "svg".into(),
                "--theme".into(),
                THEME.into(),
                "--symbol".into(),
                symbol.lib_ref.item.clone().into(),
                "--output".into(),
                out.clone().into(),
                symbol.library.clone().into(),
            ];
            self.export(kicad, &args, &out)
        })
    }

    /// The footprint, exported with `kicad-cli fp export svg`.
    ///
    /// kicad-cli loads every footprint in a `.pretty` directory before
    /// exporting one, which takes seconds for KiCad's large bundled
    /// libraries. Each footprint is a self-contained file, so the exact file
    /// is copied into a one-footprint staging library and exported from there.
    pub fn footprint(
        &self,
        kicad: &KicadInstall,
        footprint: &ResolvedFootprint,
    ) -> Result<Preview, PreviewError> {
        let library_sha256 = self.library_hash(&footprint.library)?;
        let entry = self.entry_dir(kicad, "fp", &library_sha256, &footprint.lib_ref.item);
        self.cached_or_export(entry, library_sha256, |staging| {
            let library = staging
                .join("in")
                .join(format!("{}.pretty", footprint.lib_ref.library));
            fs::create_dir_all(&library).map_err(|e| io_error("cannot create", &library, e))?;
            let file_name = format!("{}.kicad_mod", footprint.lib_ref.item);
            fs::copy(&footprint.file, library.join(&file_name))
                .map_err(|e| io_error("cannot copy", &footprint.file, e))?;
            let out = staging.join("out");
            // Unlike `sym export`, `fp export` needs its output directory to
            // exist already.
            fs::create_dir_all(&out).map_err(|e| io_error("cannot create", &out, e))?;
            let args: Vec<OsString> = vec![
                "fp".into(),
                "export".into(),
                "svg".into(),
                "--theme".into(),
                THEME.into(),
                "--footprint".into(),
                footprint.lib_ref.item.clone().into(),
                "--output".into(),
                out.clone().into(),
                library.into(),
            ];
            self.export(kicad, &args, &out)
        })
    }

    fn entry_dir(
        &self,
        kicad: &KicadInstall,
        kind: &str,
        library_sha256: &str,
        item: &str,
    ) -> PathBuf {
        self.root
            .join(CACHE_LAYOUT)
            .join(format!("kicad-{}", kicad.version))
            .join(PROFILE)
            .join(format!("{kind}-{library_sha256}"))
            .join(item_dir_name(item))
    }

    fn cached_or_export(
        &self,
        entry: PathBuf,
        library_sha256: String,
        export: impl FnOnce(&Path) -> Result<(), PreviewError>,
    ) -> Result<Preview, PreviewError> {
        if entry.is_dir() {
            return Ok(Preview {
                images: read_images(&entry)?,
                library_sha256,
                cache: CacheOutcome::Hit,
            });
        }

        let staging_root = self.root.join("staging");
        let staging = staging_root.join(format!(
            "{}-{}",
            std::process::id(),
            self.staging_counter.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging).map_err(|e| io_error("cannot create", &staging, e))?;
        let result = export(&staging).and_then(|()| {
            let parent = entry.parent().unwrap_or(&self.root);
            fs::create_dir_all(parent).map_err(|e| io_error("cannot create", parent, e))?;
            match fs::rename(staging.join("out"), &entry) {
                Ok(()) => Ok(()),
                // Another request finished the same export first.
                Err(_) if entry.is_dir() => Ok(()),
                Err(error) => Err(io_error("cannot store preview in", &entry, error)),
            }
        });
        let _ = fs::remove_dir_all(&staging);
        result?;
        Ok(Preview {
            images: read_images(&entry)?,
            library_sha256,
            cache: CacheOutcome::Exported,
        })
    }

    fn export(
        &self,
        kicad: &KicadInstall,
        args: &[OsString],
        out: &Path,
    ) -> Result<(), PreviewError> {
        let command = describe_command(args);
        self.exports.fetch_add(1, Ordering::SeqCst);
        let output = run_kicad_cli(&kicad.cli, args, EXPORT_TIMEOUT).map_err(|error| {
            PreviewError::ExportFailed {
                command: command.clone(),
                cli_message: format!("could not start {}: {error}", kicad.cli.display()),
            }
        })?;
        // `fp export` exits 0 even when it fails, so success means an SVG
        // was actually written.
        let wrote_svg = svg_files(out)
            .map(|files| !files.is_empty())
            .unwrap_or(false);
        if output.success() && wrote_svg {
            return Ok(());
        }
        Err(PreviewError::ExportFailed {
            command,
            cli_message: failure_message(&output),
        })
    }

    fn library_hash(&self, library: &Path) -> Result<String, PreviewError> {
        let fingerprint = fingerprint(library)?;
        let mut hashes = self.hashes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((known, hash)) = hashes.get(library) {
            if *known == fingerprint {
                return Ok(hash.clone());
            }
        }
        let hash = hash_library(library)?;
        hashes.insert(library.to_path_buf(), (fingerprint, hash.clone()));
        Ok(hash)
    }
}

fn failure_message(output: &CliOutput) -> String {
    let message = output.message();
    let mut text = if message.is_empty() {
        "kicad-cli wrote no SVG and printed no message.".to_owned()
    } else {
        message
    };
    if let Some(code) = output.code.filter(|code| *code != 0) {
        text.push_str(&format!("\n(exit code {code})"));
    }
    text
}

fn describe_command(args: &[OsString]) -> String {
    let mut text = String::from("kicad-cli");
    for arg in args {
        text.push(' ');
        text.push_str(&arg.to_string_lossy());
    }
    text
}

/// A readable, collision-free directory name for a symbol or footprint
/// name. The hash keeps names distinct on case-insensitive file systems.
fn item_dir_name(item: &str) -> String {
    let readable: String = item
        .chars()
        .take(48)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let digest = hex(&Sha256::digest(item.as_bytes()));
    format!("{readable}-{}", &digest[..16])
}

fn read_images(dir: &Path) -> Result<Vec<PreviewImage>, PreviewError> {
    let mut images = Vec::new();
    for file in svg_files(dir).map_err(|e| io_error("cannot read", dir, e))? {
        let svg = fs::read_to_string(&file).map_err(|e| io_error("cannot read", &file, e))?;
        images.push(PreviewImage {
            unit: unit_number(&file),
            file,
            svg,
        });
    }
    images.sort_by(|a, b| a.unit.cmp(&b.unit).then_with(|| a.file.cmp(&b.file)));
    Ok(images)
}

fn svg_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
        {
            files.push(path);
        }
    }
    Ok(files)
}

/// `kicad-cli sym export svg` names files `<SYMBOL>_unit<N>.svg`.
fn unit_number(file: &Path) -> Option<u32> {
    let stem = file.file_stem()?.to_str()?;
    let (_, unit) = stem.rsplit_once("_unit")?;
    unit.parse().ok()
}

fn fingerprint(library: &Path) -> Result<Fingerprint, PreviewError> {
    let stat = |path: &Path| {
        fs::metadata(path)
            .map(|meta| (meta.len(), meta.modified().ok()))
            .map_err(|e| io_error("cannot read", path, e))
    };
    if library.is_file() {
        let (len, modified) = stat(library)?;
        return Ok(vec![(OsString::new(), len, modified)]);
    }
    let mut entries = Vec::new();
    for path in library_files(library)? {
        let (len, modified) = stat(&path)?;
        entries.push((
            path.file_name().unwrap_or_default().to_owned(),
            len,
            modified,
        ));
    }
    Ok(entries)
}

/// SHA-256 of a `.kicad_sym` file, or of every `.kicad_mod` in a `.pretty`
/// directory (name and bytes, in name order).
fn hash_library(library: &Path) -> Result<String, PreviewError> {
    let mut hasher = Sha256::new();
    if library.is_file() {
        let bytes = fs::read(library).map_err(|e| io_error("cannot read", library, e))?;
        hasher.update(&bytes);
    } else {
        for path in library_files(library)? {
            let bytes = fs::read(&path).map_err(|e| io_error("cannot read", &path, e))?;
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            hasher.update((name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
        }
    }
    Ok(hex(&hasher.finalize()))
}

fn library_files(library: &Path) -> Result<Vec<PathBuf>, PreviewError> {
    let entries = fs::read_dir(library).map_err(|e| io_error("cannot read", library, e))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| io_error("cannot read", library, e))?
            .path();
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("kicad_mod"))
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_numbers_come_from_kicad_file_names() {
        assert_eq!(unit_number(Path::new("JETSON_NANO_unit3.svg")), Some(3));
        assert_eq!(unit_number(Path::new("R_unit1.svg")), Some(1));
        assert_eq!(unit_number(Path::new("R_0603.svg")), None);
    }

    #[test]
    fn item_directories_are_distinct_for_case_variants() {
        let upper = item_dir_name("R");
        let lower = item_dir_name("r");
        assert_ne!(upper.to_lowercase(), lower.to_lowercase());
        assert!(item_dir_name("a/b:c").starts_with("a_b_c-"));
    }

    #[test]
    fn footprint_library_hash_follows_content() {
        let dir = tempfile::tempdir().unwrap();
        let pretty = dir.path().join("x.pretty");
        fs::create_dir_all(&pretty).unwrap();
        fs::write(pretty.join("A.kicad_mod"), b"one").unwrap();
        fs::write(pretty.join("notes.txt"), b"ignored").unwrap();
        let first = hash_library(&pretty).unwrap();
        fs::write(pretty.join("notes.txt"), b"still ignored").unwrap();
        assert_eq!(hash_library(&pretty).unwrap(), first);
        fs::write(pretty.join("A.kicad_mod"), b"two").unwrap();
        assert_ne!(hash_library(&pretty).unwrap(), first);
    }
}
