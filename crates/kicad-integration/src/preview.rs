//! SVG previews exported by the installed kicad-cli, cached on disk.
//!
//! Cache layout, under the preview cache folder:
//!
//! ```text
//! <root>/v1/kicad-<version>/<theme>/<kind>-<sha256>/<item>/*.svg
//! <root>/staging/run-<process id>-<n>/        exports in progress
//! ```
//!
//! `<kind>` is `sym` or `fp` for a library on disk, keyed by the library's
//! content SHA-256, and `sym-blob` or `fp-blob` for a CAD item revision's
//! stored bytes, keyed by the blob's SHA-256. The hash and KiCad version are
//! part of every path, so an edited library or a different KiCad never
//! serves a stale image, and a repeat view never re-runs kicad-cli.
//!
//! Exports are written to a staging folder and renamed into place, so a
//! crash cannot leave a partial entry; staging folders a crash leaves behind
//! are swept the next time a cache opens ([`staging`]). Requests for the same
//! entry share one export, at most [`MAX_RUNNING`] exports run at once, and
//! waiting exports for a superseded selection are dropped ([`queue`]).
//! [`PreviewCache::evict_unused`] removes entries nothing uses any more
//! ([`eviction`]).

mod eviction;
mod queue;
mod staging;

pub use eviction::{UNUSED_FOR, remove_old_cache};
pub use queue::{MAX_RUNNING, Selection};

use crate::discovery::{KicadInstall, KicadVersion};
use crate::library_file::SymbolLibrary;
use crate::process::{CliOutput, run_kicad_cli};
use crate::resolver::{LibRef, ResolvedFootprint, ResolvedSymbol};
use queue::{Queue, Role};
use sha2::{Digest, Sha256};
use staging::RunFolder;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const CACHE_LAYOUT: &str = "v1";
/// KiCad's built-in colour theme, so exports do not depend on the user's
/// editor settings. The cache directory name is derived from it, so a theme
/// change can never serve images exported with another theme.
const THEME: &str = "_builtin_default";
const EXPORT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheOutcome {
    /// Served from the cache; kicad-cli did not run.
    Hit,
    /// kicad-cli exported the preview just now, for this request or for
    /// another request for the same item that it waited for.
    Exported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewImage {
    /// Symbol unit number (1-based); `None` for footprints.
    pub unit: Option<u32>,
    /// True for a unit's alternate (De Morgan) body style, which kicad-cli
    /// exports as `<SYMBOL>_unit<N>_demorgan.svg` next to the normal one.
    pub demorgan: bool,
    pub file: PathBuf,
    pub svg: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Preview {
    /// One image per symbol unit (normal body style, then De Morgan when the
    /// symbol has one), in unit order, or a single footprint image.
    pub images: Vec<PreviewImage>,
    /// The SHA-256 the entry is keyed by: the library's content, or the
    /// stored blob's bytes.
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
    /// The item cannot be passed to kicad-cli safely or completely.
    Unexportable(String),
    /// A CAD item revision's stored bytes could not be fetched, or were not
    /// the bytes their SHA-256 names. The message finishes the sentence
    /// "... because ...".
    StoredBytes(String),
    /// A newer selection replaced the one this request was for before its
    /// export started, so the export did not run.
    Superseded,
    Io(String),
}

impl fmt::Display for PreviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExportFailed {
                command,
                cli_message,
            } => write!(f, "`{command}` failed: {cli_message}"),
            Self::Superseded => f.write_str("a newer selection replaced this preview request"),
            Self::Unexportable(message) | Self::StoredBytes(message) | Self::Io(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for PreviewError {}

/// One CAD item revision's stored bytes, as the team catalog keeps them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredItem {
    /// The reference the part uses, such as `g-pas:R_US`.
    pub lib_ref: LibRef,
    /// The SHA-256 of the stored bytes. The bytes are checked against it.
    pub blob_sha256: String,
    /// For a symbol, the `(version ...)` of the library its entry came
    /// from; the entry is wrapped in a one-symbol library of that version.
    pub format_version: Option<String>,
}

/// Runs kicad-cli. Tests replace it to control timing.
type Runner = Arc<dyn Fn(&Path, &[OsString], Duration) -> io::Result<CliOutput> + Send + Sync>;

/// A cheap stand-in for library content: names, sizes and modification
/// times. The content hash is recomputed only when this changes.
type Fingerprint = Vec<(OsString, u64, Option<SystemTime>)>;

pub struct PreviewCache {
    root: PathBuf,
    hashes: Mutex<HashMap<PathBuf, (Fingerprint, String)>>,
    exports: AtomicUsize,
    queue: Queue,
    /// Created on the first export.
    staging: Mutex<Option<RunFolder>>,
    /// Library folders whose last-used time this process already updated.
    marked_used: Mutex<HashSet<PathBuf>>,
    runner: Runner,
}

impl PreviewCache {
    /// Opens the cache in `root`, first removing staging folders that
    /// crashed or killed apps left behind.
    pub fn new(root: PathBuf) -> Self {
        Self::with_runner(
            root,
            Arc::new(|cli, args, timeout| run_kicad_cli(cli, args, timeout)),
        )
    }

    fn with_runner(root: PathBuf, runner: Runner) -> Self {
        staging::sweep(&root.join("staging"));
        Self {
            root,
            hashes: Mutex::new(HashMap::new()),
            exports: AtomicUsize::new(0),
            queue: Queue::new(),
            staging: Mutex::new(None),
            marked_used: Mutex::new(HashSet::new()),
            runner,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// How many times this cache has run kicad-cli to export.
    pub fn export_count(&self) -> usize {
        self.exports.load(Ordering::SeqCst)
    }

    /// The most kicad-cli exports that ran at the same time so far; never
    /// more than [`MAX_RUNNING`].
    pub fn peak_running(&self) -> usize {
        self.queue.peak_running()
    }

    /// Records that a newer selection exists, such as a new click in the
    /// results list. Exports still waiting for a turn, and wanted only by
    /// older selections, are dropped. Call it as soon as a request arrives,
    /// before slower work such as fetching stored bytes.
    pub fn note_selection(&self, selection: Selection) {
        self.queue.note_selection(selection);
    }

    /// Removes entries for other KiCad versions, themes and cache layouts,
    /// and library folders not used for [`UNUSED_FOR`]. Call it once, after
    /// finding KiCad and before drawing previews. Returns what it removed.
    pub fn evict_unused(&self, kicad: &KicadVersion) -> Vec<PathBuf> {
        eviction::evict(
            &self.root,
            &eviction::InUse {
                layout: CACHE_LAYOUT,
                kicad: &kicad_dir_name(kicad),
                theme: &theme_dir_name(),
            },
        )
    }

    /// Every unit of `symbol`, exported with `kicad-cli sym export svg`.
    pub fn symbol(
        &self,
        kicad: &KicadInstall,
        symbol: &ResolvedSymbol,
    ) -> Result<Preview, PreviewError> {
        self.symbol_for(kicad, symbol, None)
    }

    /// Like [`Self::symbol`], for one `selection`.
    pub fn symbol_for(
        &self,
        kicad: &KicadInstall,
        symbol: &ResolvedSymbol,
        selection: Selection,
    ) -> Result<Preview, PreviewError> {
        let name = &symbol.lib_ref.item;
        reject_option_like(name)?;
        let library_sha256 = self.library_hash(&symbol.library)?;
        let entry = self.entry_dir(kicad, "sym", &library_sha256, name);
        let export = Export {
            prepare: |_: &Path| Ok(()),
            run: |staging: &Path| self.export_symbol(kicad, name, &symbol.library, staging),
        };
        self.cached_or_export(entry, library_sha256, selection, export)
    }

    /// The footprint, exported with `kicad-cli fp export svg`.
    pub fn footprint(
        &self,
        kicad: &KicadInstall,
        footprint: &ResolvedFootprint,
    ) -> Result<Preview, PreviewError> {
        self.footprint_for(kicad, footprint, None)
    }

    /// Like [`Self::footprint`], for one `selection`.
    ///
    /// kicad-cli loads every footprint in a `.pretty` directory before
    /// exporting one, which takes seconds for KiCad's large bundled
    /// libraries. Each footprint is a self-contained file, so the exact file
    /// is copied into a one-footprint staging library and exported from there.
    pub fn footprint_for(
        &self,
        kicad: &KicadInstall,
        footprint: &ResolvedFootprint,
        selection: Selection,
    ) -> Result<Preview, PreviewError> {
        let lib_ref = &footprint.lib_ref;
        reject_option_like(&lib_ref.item)?;
        let library_sha256 = self.library_hash(&footprint.library)?;
        let entry = self.entry_dir(kicad, "fp", &library_sha256, &lib_ref.item);
        let export = Export {
            prepare: |staging: &Path| {
                let bytes = fs::read(&footprint.file)
                    .map_err(|e| io_error("cannot read", &footprint.file, e))?;
                write_footprint_library(staging, lib_ref, &bytes).map(|_| ())
            },
            run: |staging: &Path| {
                self.export_footprint(
                    kicad,
                    lib_ref,
                    &footprint_library(staging, lib_ref),
                    staging,
                )
            },
        };
        self.cached_or_export(entry, library_sha256, selection, export)
    }

    /// Every unit of a symbol from a CAD item revision's stored bytes: one
    /// top-level `(symbol ...)` entry, wrapped in a one-symbol library.
    /// `load_bytes` runs only when the cache does not have the preview yet.
    pub fn stored_symbol(
        &self,
        kicad: &KicadInstall,
        item: &StoredItem,
        load_bytes: impl FnOnce() -> Result<Vec<u8>, String>,
        selection: Selection,
    ) -> Result<Preview, PreviewError> {
        let name = &item.lib_ref.item;
        reject_option_like(name)?;
        let Some(version) = item.format_version.as_deref() else {
            return Err(PreviewError::Unexportable(format!(
                "The stored revision of {} does not say which KiCad file version it came from, so it cannot be put in a library.",
                item.lib_ref
            )));
        };
        // The same entry can be wrapped in libraries of different versions.
        let entry = self.entry_dir(
            kicad,
            "sym-blob",
            &item.blob_sha256,
            &format!("{name} version {version}"),
        );
        let export = Export {
            prepare: |staging: &Path| {
                let bytes = checked_bytes(item, load_bytes)?;
                write_symbol_library(staging, item, version, &bytes)
            },
            run: |staging: &Path| {
                let library = symbol_library(staging, &item.lib_ref);
                self.export_symbol(kicad, name, &library, staging)
            },
        };
        self.cached_or_export(entry, item.blob_sha256.clone(), selection, export)
    }

    /// A footprint from a CAD item revision's stored bytes: a whole
    /// `.kicad_mod`, written into a one-footprint library. `load_bytes` runs
    /// only when the cache does not have the preview yet.
    pub fn stored_footprint(
        &self,
        kicad: &KicadInstall,
        item: &StoredItem,
        load_bytes: impl FnOnce() -> Result<Vec<u8>, String>,
        selection: Selection,
    ) -> Result<Preview, PreviewError> {
        let lib_ref = &item.lib_ref;
        reject_option_like(&lib_ref.item)?;
        let entry = self.entry_dir(kicad, "fp-blob", &item.blob_sha256, &lib_ref.item);
        let export = Export {
            prepare: |staging: &Path| {
                let bytes = checked_bytes(item, load_bytes)?;
                write_footprint_library(staging, lib_ref, &bytes).map(|_| ())
            },
            run: |staging: &Path| {
                self.export_footprint(
                    kicad,
                    lib_ref,
                    &footprint_library(staging, lib_ref),
                    staging,
                )
            },
        };
        self.cached_or_export(entry, item.blob_sha256.clone(), selection, export)
    }

    fn entry_dir(&self, kicad: &KicadInstall, kind: &str, sha256: &str, item: &str) -> PathBuf {
        self.root
            .join(CACHE_LAYOUT)
            .join(kicad_dir_name(&kicad.version))
            .join(theme_dir_name())
            .join(format!("{kind}-{sha256}"))
            .join(item_dir_name(item))
    }

    /// Serves `entry` from the cache, or exports it. Requests for the same
    /// entry share one export; see [`queue`].
    fn cached_or_export(
        &self,
        entry: PathBuf,
        library_sha256: String,
        selection: Selection,
        export: Export<
            impl FnOnce(&Path) -> Result<(), PreviewError>,
            impl FnOnce(&Path) -> Result<(), PreviewError>,
        >,
    ) -> Result<Preview, PreviewError> {
        if entry.is_dir() {
            return self.served(&entry, library_sha256, CacheOutcome::Hit);
        }
        self.queue.note_selection(selection);
        let mut leader = match self.queue.join(&entry, selection) {
            Role::Follower(flight) => {
                flight.wait()?;
                return self.served(&entry, library_sha256, CacheOutcome::Exported);
            }
            Role::Leader(leader) => leader,
        };
        // Another export may have finished between the check and joining.
        if entry.is_dir() {
            leader.finish(Ok(()));
            return self.served(&entry, library_sha256, CacheOutcome::Hit);
        }
        let outcome = self.export_entry(&entry, &mut leader, export);
        leader.finish(outcome.clone());
        outcome?;
        self.served(&entry, library_sha256, CacheOutcome::Exported)
    }

    /// Prepares the export's input, waits for a turn, runs kicad-cli, and
    /// renames the output into `entry`.
    fn export_entry(
        &self,
        entry: &Path,
        leader: &mut queue::Leader<'_>,
        export: Export<
            impl FnOnce(&Path) -> Result<(), PreviewError>,
            impl FnOnce(&Path) -> Result<(), PreviewError>,
        >,
    ) -> Result<(), PreviewError> {
        let staging = self.staging_folder()?;
        let result = (export.prepare)(&staging).and_then(|()| {
            let Some(turn) = leader.wait_for_turn() else {
                return Err(PreviewError::Superseded);
            };
            let ran = (export.run)(&staging);
            drop(turn);
            ran?;
            let parent = entry.parent().unwrap_or(&self.root);
            fs::create_dir_all(parent).map_err(|e| io_error("cannot create", parent, e))?;
            match fs::rename(staging.join("out"), entry) {
                Ok(()) => Ok(()),
                // Another app finished the same export first.
                Err(_) if entry.is_dir() => Ok(()),
                Err(error) => Err(io_error("cannot store preview in", entry, error)),
            }
        });
        let _ = fs::remove_dir_all(&staging);
        result
    }

    fn staging_folder(&self) -> Result<PathBuf, PreviewError> {
        let staging_root = self.root.join("staging");
        let mut run = self.staging.lock().unwrap_or_else(|e| e.into_inner());
        if run.is_none() {
            *run = Some(
                RunFolder::create(&staging_root)
                    .map_err(|e| io_error("cannot create", &staging_root, e))?,
            );
        }
        let run = run.as_ref().expect("the run folder was just created");
        run.export_folder()
            .map_err(|e| io_error("cannot create a folder in", &staging_root, e))
    }

    /// Reads a stored entry and records that its library folder was used.
    fn served(
        &self,
        entry: &Path,
        library_sha256: String,
        cache: CacheOutcome,
    ) -> Result<Preview, PreviewError> {
        if let Some(library) = entry.parent() {
            self.mark_used(library);
        }
        Ok(Preview {
            images: read_images(entry)?,
            library_sha256,
            cache,
        })
    }

    /// Updates the library folder's last-used time, once per process, so
    /// eviction keeps folders in use.
    fn mark_used(&self, library: &Path) {
        let mut marked = self.marked_used.lock().unwrap_or_else(|e| e.into_inner());
        if marked.insert(library.to_path_buf()) {
            let _ = fs::write(library.join(eviction::LAST_USED), b"");
        }
    }

    fn export_symbol(
        &self,
        kicad: &KicadInstall,
        name: &str,
        library: &Path,
        staging: &Path,
    ) -> Result<(), PreviewError> {
        let out = staging.join("out");
        let args: Vec<OsString> = vec![
            "sym".into(),
            "export".into(),
            "svg".into(),
            "--theme".into(),
            THEME.into(),
            "--symbol".into(),
            name.into(),
            "--output".into(),
            out.clone().into(),
            library.into(),
        ];
        let stem = output_stem(name);
        self.run_export(kicad, &args, &out, |file| {
            file.strip_prefix(stem.as_str())
                .and_then(|rest| rest.strip_suffix(".svg"))
                .is_some_and(|rest| parse_unit_suffix(rest).is_some())
        })
    }

    fn export_footprint(
        &self,
        kicad: &KicadInstall,
        lib_ref: &LibRef,
        library: &Path,
        staging: &Path,
    ) -> Result<(), PreviewError> {
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
            lib_ref.item.clone().into(),
            "--output".into(),
            out.clone().into(),
            library.into(),
        ];
        let expected = format!("{}.svg", output_stem(&lib_ref.item));
        self.run_export(kicad, &args, &out, |file| file == expected)
    }

    /// Runs one export. `expected` decides whether an output file name
    /// belongs to the requested item; anything else fails the export.
    fn run_export(
        &self,
        kicad: &KicadInstall,
        args: &[OsString],
        out: &Path,
        expected: impl Fn(&str) -> bool,
    ) -> Result<(), PreviewError> {
        let command = describe_command(args);
        self.exports.fetch_add(1, Ordering::SeqCst);
        let output = (self.runner)(&kicad.cli, args, EXPORT_TIMEOUT).map_err(|error| {
            PreviewError::ExportFailed {
                command: command.clone(),
                cli_message: format!("could not start {}: {error}", kicad.cli.display()),
            }
        })?;
        // `fp export` exits 0 even when it fails, so success means an SVG
        // was actually written, and only for the requested item.
        let names: Vec<String> = svg_files(out)
            .unwrap_or_default()
            .iter()
            .filter_map(|file| file.file_name()?.to_str().map(str::to_owned))
            .collect();
        let unexpected: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|name| !expected(name))
            .take(5)
            .collect();
        if !unexpected.is_empty() {
            return Err(PreviewError::ExportFailed {
                command,
                cli_message: format!(
                    "kicad-cli wrote files for something other than the requested item: {}",
                    unexpected.join(", ")
                ),
            });
        }
        if output.success() && !names.is_empty() {
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

/// The two steps of an export. `prepare` writes the input into the staging
/// folder before waiting for a turn, so slow work such as fetching stored
/// bytes never holds one; `run` runs kicad-cli.
struct Export<P, R> {
    prepare: P,
    run: R,
}

/// The SHA-256 of `bytes`, as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Loads a stored item's bytes and checks them against its SHA-256.
fn checked_bytes(
    item: &StoredItem,
    load_bytes: impl FnOnce() -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, PreviewError> {
    let bytes = load_bytes().map_err(PreviewError::StoredBytes)?;
    if sha256_hex(&bytes) != item.blob_sha256 {
        return Err(PreviewError::StoredBytes(
            "the bytes the team server sent do not match their SHA-256".into(),
        ));
    }
    Ok(bytes)
}

/// Writes `entry` (one top-level `(symbol ...)`) as the only symbol of a
/// library of `version`, and checks that kicad-cli can draw it from there.
fn write_symbol_library(
    staging: &Path,
    item: &StoredItem,
    version: &str,
    entry: &[u8],
) -> Result<(), PreviewError> {
    let mut library =
        format!("(kicad_symbol_lib\n\t(version {version})\n\t(generator \"kicad_parts\")\n\t")
            .into_bytes();
    library.extend_from_slice(entry);
    library.extend_from_slice(b"\n)\n");
    let parsed = SymbolLibrary::parse(&library).map_err(|error| {
        PreviewError::Unexportable(format!(
            "The stored revision of {} is not a symbol KiCad can read: {error}",
            item.lib_ref
        ))
    })?;
    match parsed.entries.as_slice() {
        [only] if only.name == item.lib_ref.item => {
            if let Some(parent) = &only.extends {
                return Err(PreviewError::Unexportable(format!(
                    "The stored revision of {} is based on the symbol {parent}, which it does not include, so kicad-cli cannot draw it.",
                    item.lib_ref
                )));
            }
        }
        _ => {
            return Err(PreviewError::Unexportable(format!(
                "The stored revision of {} is not exactly one symbol named {}.",
                item.lib_ref, item.lib_ref.item
            )));
        }
    }
    let path = symbol_library(staging, &item.lib_ref);
    let folder = staging.join("in");
    fs::create_dir_all(&folder).map_err(|e| io_error("cannot create", &folder, e))?;
    fs::write(&path, library).map_err(|e| io_error("cannot write", &path, e))
}

fn symbol_library(staging: &Path, lib_ref: &LibRef) -> PathBuf {
    staging
        .join("in")
        .join(format!("{}.kicad_sym", lib_ref.library))
}

/// Writes one `.kicad_mod` as the only footprint of a staging library.
fn write_footprint_library(
    staging: &Path,
    lib_ref: &LibRef,
    bytes: &[u8],
) -> Result<PathBuf, PreviewError> {
    let library = footprint_library(staging, lib_ref);
    fs::create_dir_all(&library).map_err(|e| io_error("cannot create", &library, e))?;
    let file = library.join(format!("{}.kicad_mod", lib_ref.item));
    fs::write(&file, bytes).map_err(|e| io_error("cannot write", &file, e))?;
    Ok(library)
}

fn footprint_library(staging: &Path, lib_ref: &LibRef) -> PathBuf {
    staging
        .join("in")
        .join(format!("{}.pretty", lib_ref.library))
}

fn kicad_dir_name(version: &KicadVersion) -> String {
    format!("kicad-{version}")
}

fn theme_dir_name() -> String {
    format!("theme{THEME}")
}

fn io_error(context: &str, path: &Path, error: io::Error) -> PreviewError {
    PreviewError::Io(format!("{context} {}: {error}", path.display()))
}

/// The file-name stem kicad-cli writes for `name`: it replaces characters
/// Windows forbids in file names (`*`, `?`, `|`, and `<`, `>`, `"`, which
/// KiCad libraries cannot contain anyway) with a space, so `A*B` is written
/// as `A B_unit1.svg`.
fn output_stem(name: &str) -> String {
    name.chars()
        .map(|c| {
            if matches!(c, '*' | '?' | '|' | '<' | '>' | '"') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// kicad-cli parses a leading `-` as an option, so such a name would either
/// fail ("Unknown argument") or change what is exported.
fn reject_option_like(name: &str) -> Result<(), PreviewError> {
    if name.starts_with('-') {
        return Err(PreviewError::Unexportable(format!(
            "kicad-cli cannot export names beginning with '-' (`{name}`)."
        )));
    }
    Ok(())
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
    let digest = sha256_hex(item.as_bytes());
    format!("{readable}-{}", &digest[..16])
}

fn read_images(dir: &Path) -> Result<Vec<PreviewImage>, PreviewError> {
    let mut images = Vec::new();
    for file in svg_files(dir).map_err(|e| io_error("cannot read", dir, e))? {
        let svg = fs::read_to_string(&file).map_err(|e| io_error("cannot read", &file, e))?;
        let (unit, demorgan) = file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.rfind("_unit").map(|at| &stem[at..]))
            .and_then(parse_unit_suffix)
            .map_or((None, false), |(unit, demorgan)| (Some(unit), demorgan));
        images.push(PreviewImage {
            unit,
            demorgan,
            file,
            svg,
        });
    }
    images.sort_by(|a, b| {
        (a.unit, a.demorgan)
            .cmp(&(b.unit, b.demorgan))
            .then_with(|| a.file.cmp(&b.file))
    });
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

/// Parses the `_unit<N>` or `_unit<N>_demorgan` that kicad-cli appends to a
/// symbol name, returning the unit and whether it is the De Morgan body.
fn parse_unit_suffix(suffix: &str) -> Option<(u32, bool)> {
    let rest = suffix.strip_prefix("_unit")?;
    let (digits, demorgan) = match rest.strip_suffix("_demorgan") {
        Some(digits) => (digits, true),
        None => (rest, false),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((digits.parse().ok()?, demorgan))
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
mod tests;
