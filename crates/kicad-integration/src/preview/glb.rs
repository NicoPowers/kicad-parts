//! 3D views (kicad-parts-c9c1): a one-footprint board exported as GLB by
//! `kicad-cli pcb export glb`, and copies of 3D model files the board
//! points at.
//!
//! A 3D view is a cache entry like a preview, in the same folders, so
//! eviction and staging treat it the same way:
//!
//! ```text
//! <theme>/glb-<sha256>/<footprint>/board.glb     the 3D view
//! <theme>/glb-<sha256>/<footprint>/kicad-cli.txt what kicad-cli printed
//! <theme>/model-<sha256>/model/model.<ext>        a stored or chosen model
//! ```
//!
//! A 3D view's SHA-256 covers the board's text and every model file's path
//! and content, and the KiCad version is part of the path. So a changed
//! model file makes a new 3D view. 3D exports wait in the same queue as
//! previews: one export per entry, [`super::MAX_RUNNING`] at once, and
//! dropped when only older selections want them.

use super::*;

/// The GLB file in a 3D view's entry.
const GLB_FILE: &str = "board.glb";
/// What kicad-cli printed, kept for the next time the entry is served.
const MESSAGES_FILE: &str = "kicad-cli.txt";
/// The board file kicad-cli reads, in the staging folder.
const BOARD_FILE: &str = "board.kicad_pcb";

/// One board to export as a 3D view.
pub struct BoardExport<'a> {
    /// The footprint's name, for the entry's folder name.
    pub name: &'a str,
    /// The `.kicad_pcb` text.
    pub board: &'a str,
    /// Every 3D model file the board points at.
    pub models: &'a [PathBuf],
}

/// A 3D view, ready in the cache.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Glb {
    /// What [`PreviewCache::glb_bytes`] reads the GLB by.
    pub key: String,
    /// The GLB's size in bytes.
    pub size: u64,
    pub cache: CacheOutcome,
    /// What kicad-cli printed when it exported it, such as a model it
    /// could not add.
    pub messages: String,
}

impl PreviewCache {
    /// The board's 3D view, exported with
    /// `kicad-cli pcb export glb --include-pads --include-silkscreen
    /// --include-soldermask`, or served from the cache.
    pub fn board_glb(
        &self,
        kicad: &KicadInstall,
        export: &BoardExport<'_>,
        selection: Selection,
    ) -> Result<Glb, PreviewError> {
        let mut hasher = Sha256::new();
        hash_part(&mut hasher, export.board.as_bytes());
        for model in export.models {
            hash_part(&mut hasher, model.to_string_lossy().as_bytes());
            hash_part(&mut hasher, self.file_sha256(model)?.as_bytes());
        }
        let sha256 = hex(&hasher.finalize());
        let entry = self.entry_dir(kicad, "glb", &sha256, export.name);
        let steps = Export {
            prepare: |staging: &Path| {
                let folder = staging.join("in");
                fs::create_dir_all(&folder).map_err(|e| io_error("cannot create", &folder, e))?;
                let board = folder.join(BOARD_FILE);
                fs::write(&board, export.board).map_err(|e| io_error("cannot write", &board, e))
            },
            run: |staging: &Path| self.export_glb(kicad, staging),
        };
        let served = self.cached_or_export(entry.clone(), sha256, selection, steps)?;
        let glb = entry.join(GLB_FILE);
        let size = fs::metadata(&glb)
            .map_err(|e| io_error("cannot read", &glb, e))?
            .len();
        Ok(Glb {
            key: self.entry_key(&entry).ok_or_else(|| {
                PreviewError::Io(format!("{} is outside the cache", entry.display()))
            })?,
            size,
            cache: served.cache,
            messages: fs::read_to_string(entry.join(MESSAGES_FILE)).unwrap_or_default(),
        })
    }

    /// The GLB of a 3D view, by the key its [`Glb`] gave. `None` for a key
    /// that is not exactly a 3D view entry of this cache.
    pub fn glb_bytes(&self, key: &str) -> Option<Vec<u8>> {
        fs::read(self.entry_by_key(key, "glb-")?.join(GLB_FILE)).ok()
    }

    /// The SHA-256 of a file, worked out again only when its size or time
    /// changes.
    pub fn file_sha256(&self, file: &Path) -> Result<String, PreviewError> {
        if !file.is_file() {
            return Err(PreviewError::Io(format!(
                "{} is not a file",
                file.display()
            )));
        }
        self.library_hash(file)
    }

    /// A copy in the cache of the 3D model file `name` whose bytes have
    /// `sha256`, for a board to point at. The copy is named
    /// `model.<extension>`, in a folder for the bytes and the name, so the
    /// same bytes stored as `.stp` and `.step` both work. `load_bytes`
    /// runs only when the cache has no copy yet, and its bytes must match
    /// `sha256`.
    pub fn model_copy(
        &self,
        kicad: &KicadInstall,
        sha256: &str,
        name: &str,
        load_bytes: impl FnOnce() -> Result<Vec<u8>, String>,
    ) -> Result<PathBuf, PreviewError> {
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(PreviewError::StoredBytes(
                "the 3D model's SHA-256 is not 64 lowercase hex digits".into(),
            ));
        }
        let extension = Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|extension| MODEL_EXTENSIONS.contains(&extension.as_str()));
        let Some(extension) = extension else {
            return Err(PreviewError::Unexportable(format!(
                "{name:?} is not a 3D model file: KiCad's are .step, .stp, .stpz, .wrl or .wrz."
            )));
        };
        let entry = self.entry_dir(kicad, "model", sha256, name);
        let file_name = format!("model.{extension}");
        if entry.join(&file_name).is_file() {
            if let Some(library) = entry.parent() {
                self.mark_used(library);
            }
            return Ok(entry.join(file_name));
        }
        let bytes = load_bytes().map_err(PreviewError::StoredBytes)?;
        if sha256_hex(&bytes) != sha256 {
            return Err(PreviewError::StoredBytes(
                "the 3D model's bytes do not match their SHA-256".into(),
            ));
        }
        let staging = self.staging_folder()?;
        let result = (|| {
            let out = staging.join("out");
            fs::create_dir_all(&out).map_err(|e| io_error("cannot create", &out, e))?;
            let file = out.join(&file_name);
            fs::write(&file, &bytes).map_err(|e| io_error("cannot write", &file, e))?;
            let parent = entry.parent().unwrap_or(&self.root);
            fs::create_dir_all(parent).map_err(|e| io_error("cannot create", parent, e))?;
            match fs::rename(&out, &entry) {
                Ok(()) => Ok(()),
                // Another request stored the same bytes first.
                Err(_) if entry.join(&file_name).is_file() => Ok(()),
                Err(error) => Err(io_error("cannot store a 3D model in", &entry, error)),
            }
        })();
        let _ = fs::remove_dir_all(&staging);
        result?;
        if let Some(library) = entry.parent() {
            self.mark_used(library);
        }
        Ok(entry.join(file_name))
    }

    fn export_glb(&self, kicad: &KicadInstall, staging: &Path) -> Result<(), PreviewError> {
        let out = staging.join("out");
        fs::create_dir_all(&out).map_err(|e| io_error("cannot create", &out, e))?;
        let glb = out.join(GLB_FILE);
        let args: Vec<OsString> = vec![
            "pcb".into(),
            "export".into(),
            "glb".into(),
            "--include-pads".into(),
            "--include-silkscreen".into(),
            "--include-soldermask".into(),
            "--output".into(),
            glb.clone().into(),
            staging.join("in").join(BOARD_FILE).into(),
        ];
        let command = describe_command(&args);
        self.exports.fetch_add(1, Ordering::SeqCst);
        let output = (self.runner)(kicad, &args, EXPORT_TIMEOUT).map_err(|error| {
            PreviewError::ExportFailed {
                command: command.clone(),
                cli_message: error.message(&kicad.cli),
            }
        })?;
        // kicad-cli can end without an error and write nothing, so the
        // export counts only when a GLB was written. It ends with code 2
        // when it could not read a 3D model, yet still writes the board;
        // the 3D view then says which model it could not read.
        let written = starts_as_glb(&glb);
        let model_trouble =
            output.code == Some(2) && !output.timed_out && only_model_complaints(&output.message());
        if !(output.success() || model_trouble) || !written {
            let mut cli_message = output.message();
            if cli_message.is_empty() {
                cli_message = "kicad-cli wrote no 3D view and printed no message.".into();
            }
            if let Some(code) = output.code.filter(|code| *code != 0) {
                cli_message.push_str(&format!("\n(exit code {code})"));
            }
            return Err(PreviewError::ExportFailed {
                command,
                cli_message,
            });
        }
        let messages = out.join(MESSAGES_FILE);
        fs::write(&messages, output.message()).map_err(|e| io_error("cannot write", &messages, e))
    }
}

/// The 3D model file types KiCad reads, in lower case.
const MODEL_EXTENSIONS: [&str; 5] = ["step", "stp", "stpz", "wrl", "wrz"];

/// What kicad-cli prints about a 3D model it could not load. It goes on
/// without the model.
const MODEL_COMPLAINTS: [&str; 4] = [
    "readSTEP() failed",
    "No model for filename",
    "Could not add 3D model",
    "File not found",
];

/// Whether `message` holds a complaint about loading a 3D model, and
/// nothing else but kicad-cli's usual lines and the STEP reader's errors.
pub(crate) fn only_model_complaints(message: &str) -> bool {
    let lines: Vec<&str> = message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let complaint = |line: &str| MODEL_COMPLAINTS.iter().any(|known| line.contains(known));
    lines.iter().any(|line| complaint(line))
        && lines.iter().all(|line| {
            complaint(line)
                || line.starts_with("****")
                || line.starts_with("Binary GLTF file")
                || line.starts_with("Export time")
        })
}

/// Whether the file at `path` is longer than a GLB header and starts as
/// one; only the first bytes are read.
fn starts_as_glb(path: &Path) -> bool {
    use std::io::Read;
    let mut header = [0u8; 12];
    fs::metadata(path).is_ok_and(|metadata| metadata.len() > 12)
        && fs::File::open(path)
            .and_then(|mut file| file.read_exact(&mut header))
            .is_ok()
        && header.starts_with(b"glTF")
}

/// Adds `bytes` to `hasher` after its length, so parts never run together.
fn hash_part(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}
