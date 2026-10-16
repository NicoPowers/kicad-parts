use super::glb::{only_model_complaints, whole_glb};
use super::*;
use crate::discovery::InstallSource;
use crate::process::{CliSettingsFolder, RunError};
use crate::resolver::LibraryOrigin;
use std::sync::Condvar;
use std::thread;
use std::time::Instant;

#[test]
fn output_stems_follow_kicad_cli_file_naming() {
    assert_eq!(output_stem("A*B"), "A B");
    assert_eq!(output_stem("A?B|C"), "A B C");
    assert_eq!(output_stem("R_US"), "R_US");
}

#[test]
fn unit_suffixes_come_from_kicad_file_names() {
    assert_eq!(parse_unit_suffix("_unit3"), Some((3, false)));
    assert_eq!(parse_unit_suffix("_unit12_demorgan"), Some((12, true)));
    assert_eq!(parse_unit_suffix("_unit"), None);
    assert_eq!(parse_unit_suffix("_unit1_other"), None);
    assert_eq!(parse_unit_suffix("_0603"), None);
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

/// A stand-in for kicad-cli that writes the SVG kicad-cli would, waits
/// while its gate is closed, and records how many copies run at once.
#[derive(Default)]
struct FakeCli {
    gate_open: Mutex<bool>,
    gate_changed: Condvar,
    started: Mutex<Vec<String>>,
    running: AtomicUsize,
    peak: AtomicUsize,
}

impl FakeCli {
    fn new(open: bool) -> Arc<Self> {
        let fake = Arc::new(Self::default());
        *fake.gate_open.lock().unwrap() = open;
        fake
    }

    fn open_gate(&self) {
        *self.gate_open.lock().unwrap() = true;
        self.gate_changed.notify_all();
    }

    fn started(&self) -> Vec<String> {
        self.started.lock().unwrap().clone()
    }

    fn wait_until_started(&self, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.started().len() < count {
            assert!(
                Instant::now() < deadline,
                "only {:?} started",
                self.started()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn run(&self, args: &[OsString]) -> io::Result<CliOutput> {
        let value = |flag: &str| {
            let at = args.iter().position(|arg| arg == flag)?;
            Some(args.get(at + 1)?.to_string_lossy().into_owned())
        };
        // A 3D export names no item; its board is its only input.
        let name = value("--symbol")
            .or_else(|| value("--footprint"))
            .unwrap_or_else(|| "board".to_owned());
        let out = PathBuf::from(value("--output").unwrap());
        self.started.lock().unwrap().push(name.clone());
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        let mut open = self.gate_open.lock().unwrap();
        while !*open {
            open = self.gate_changed.wait(open).unwrap();
        }
        drop(open);
        // Long enough for other requests to pile up behind this one.
        thread::sleep(Duration::from_millis(30));
        if args.first().is_some_and(|arg| arg == "pcb") {
            // `--output` names the GLB file itself.
            fs::write(&out, fake_glb())?;
            self.running.fetch_sub(1, Ordering::SeqCst);
            return Ok(CliOutput {
                code: Some(0),
                stdout: "Binary GLTF file created.".into(),
                stderr: String::new(),
                timed_out: false,
            });
        }
        fs::create_dir_all(&out)?;
        let file = if value("--symbol").is_some() {
            format!("{name}_unit1.svg")
        } else {
            format!("{name}.svg")
        };
        fs::write(out.join(file), format!("<svg>{name}</svg>"))?;
        self.running.fetch_sub(1, Ordering::SeqCst);
        Ok(CliOutput {
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        })
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    library: PathBuf,
    kicad: KicadInstall,
    cache: Arc<PreviewCache>,
    fake: Arc<FakeCli>,
}

fn fixture(gate_open: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("t.kicad_sym");
    fs::write(&library, "(kicad_symbol_lib)").unwrap();
    let fake = FakeCli::new(gate_open);
    let runner: Runner = {
        let fake = fake.clone();
        Arc::new(move |_, args, _| fake.run(args).map_err(RunError::Start))
    };
    Fixture {
        cache: Arc::new(PreviewCache::with_runner(dir.path().join("cache"), runner)),
        kicad: KicadInstall {
            cli: PathBuf::from("kicad-cli"),
            version: KicadVersion {
                major: 10,
                minor: 0,
                patch: 4,
            },
            source: InstallSource::Discovered,
            share_dir: None,
            settings: CliSettingsFolder::new(dir.path().join("kicad-cli-settings")),
        },
        library,
        fake,
        _dir: dir,
    }
}

impl Fixture {
    fn symbol(&self, name: &str) -> ResolvedSymbol {
        ResolvedSymbol {
            lib_ref: LibRef {
                library: "t".into(),
                item: name.into(),
            },
            library: self.library.clone(),
            origin: LibraryOrigin::Project,
        }
    }

    fn wait_until_joined(&self, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.cache.queue.joins() < count {
            assert!(
                Instant::now() < deadline,
                "only {} joined",
                self.cache.queue.joins()
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Requests `name` for `selection` on its own thread.
    fn request(
        &self,
        name: &str,
        selection: Selection,
    ) -> thread::JoinHandle<Result<Preview, PreviewError>> {
        let (cache, kicad, symbol) = (self.cache.clone(), self.kicad.clone(), self.symbol(name));
        thread::spawn(move || cache.symbol_for(&kicad, &symbol, selection))
    }
}

#[test]
fn requests_for_the_same_item_share_one_export() {
    let fixture = fixture(false);
    let requests: Vec<_> = (0..6).map(|_| fixture.request("R", None)).collect();
    fixture.fake.wait_until_started(1);
    // All six have joined: one runs the export, five wait on it.
    fixture.wait_until_joined(6);
    fixture.fake.open_gate();
    let previews: Vec<Preview> = requests
        .into_iter()
        .map(|request| request.join().unwrap().unwrap())
        .collect();
    assert_eq!(fixture.fake.started(), ["R"]);
    assert_eq!(fixture.cache.export_count(), 1);
    assert!(
        previews
            .iter()
            .all(|preview| preview.images == previews[0].images)
    );
    // A later view is a cache hit.
    let again = fixture.request("R", None).join().unwrap().unwrap();
    assert_eq!(again.cache, CacheOutcome::Hit);
    assert_eq!(fixture.cache.export_count(), 1);
}

#[test]
fn no_more_than_the_limit_of_exports_run_at_once() {
    let fixture = fixture(true);
    let names: Vec<String> = (0..8).map(|n| format!("S{n}")).collect();
    let requests: Vec<_> = names
        .iter()
        .map(|name| fixture.request(name, None))
        .collect();
    for request in requests {
        request.join().unwrap().unwrap();
    }
    assert_eq!(fixture.cache.export_count(), 8);
    assert_eq!(fixture.fake.peak.load(Ordering::SeqCst), MAX_RUNNING);
    assert_eq!(fixture.cache.peak_running(), MAX_RUNNING);
}

#[test]
fn waiting_exports_for_an_older_selection_are_dropped() {
    let fixture = fixture(false);
    // Selection 1 fills every turn, and more of its work waits behind it.
    let running: Vec<_> = (0..MAX_RUNNING)
        .map(|n| fixture.request(&format!("Busy{n}"), Some(1)))
        .collect();
    fixture.fake.wait_until_started(MAX_RUNNING);
    let stale = fixture.request("Stale", Some(1));
    // A request without a selection is never replaced.
    let unselected = fixture.request("Unselected", None);
    fixture.wait_until_joined(MAX_RUNNING + 2);

    // The person selects another part. The waiting export is dropped at
    // once, before any turn frees up.
    let newer = fixture.request("Newer", Some(2));
    assert_eq!(stale.join().unwrap(), Err(PreviewError::Replaced));

    fixture.fake.open_gate();
    // Exports that had started finish and are cached.
    for request in running {
        assert_eq!(
            request.join().unwrap().unwrap().cache,
            CacheOutcome::Exported
        );
    }
    assert_eq!(newer.join().unwrap().unwrap().cache, CacheOutcome::Exported);
    unselected.join().unwrap().unwrap();
    let mut started = fixture.fake.started();
    started.sort();
    assert_eq!(started, ["Busy0", "Busy1", "Newer", "Unselected"]);

    // The dropped item exports normally when it is selected again.
    let again = fixture.request("Stale", Some(3)).join().unwrap().unwrap();
    assert_eq!(again.cache, CacheOutcome::Exported);
}

#[test]
fn a_newer_selection_waiting_for_the_same_item_keeps_its_export() {
    let fixture = fixture(false);
    let running: Vec<_> = (0..MAX_RUNNING)
        .map(|n| fixture.request(&format!("Busy{n}"), Some(1)))
        .collect();
    fixture.fake.wait_until_started(MAX_RUNNING);
    // Two parts share the symbol `Shared`: selection 1 queues it, then the
    // person selects the other part, which joins the same export.
    let older = fixture.request("Shared", Some(1));
    fixture.wait_until_joined(MAX_RUNNING + 1);
    let newer = fixture.request("Shared", Some(2));
    // The newer request has joined while the export still waits for a
    // turn, and selection 2 is now the newest; the export must survive.
    fixture.wait_until_joined(MAX_RUNNING + 2);
    assert_eq!(fixture.cache.queue.exports_in_progress(), MAX_RUNNING + 1);
    fixture.fake.open_gate();
    for request in running {
        request.join().unwrap().unwrap();
    }
    assert!(older.join().unwrap().is_ok());
    assert!(newer.join().unwrap().is_ok());
    let shared = fixture
        .fake
        .started()
        .iter()
        .filter(|name| *name == "Shared")
        .count();
    assert_eq!(shared, 1);
}

#[test]
fn staging_folders_are_swept_when_a_cache_opens() {
    let dir = tempfile::tempdir().unwrap();
    let staging_root = dir.path().join("staging");
    let crashed = staging_root.join("run-1-0");
    fs::create_dir_all(crashed.join("0").join("out")).unwrap();
    fs::write(crashed.join("in-use.lock"), "").unwrap();
    staging::make_older(&crashed, Duration::from_secs(2 * 24 * 60 * 60));
    let cache = PreviewCache::new(dir.path().to_path_buf());
    assert!(!crashed.exists());
    // An open cache's own folder survives another cache opening.
    let own = cache.staging_folder().unwrap();
    let _other = PreviewCache::new(dir.path().to_path_buf());
    #[cfg(windows)]
    assert!(own.parent().unwrap().exists());
    drop(cache);
    assert!(!own.parent().unwrap().exists());
}

#[test]
fn eviction_keeps_the_kicad_version_in_use_and_libraries_just_used() {
    let fixture = fixture(true);
    fixture.request("R", None).join().unwrap().unwrap();
    let older_kicad = fixture.cache.root().join(CACHE_LAYOUT).join("kicad-9.0.0");
    fs::create_dir_all(older_kicad.join("theme_builtin_default")).unwrap();
    let removed = fixture.cache.evict_unused(&fixture.kicad.version);
    assert_eq!(removed, [older_kicad]);
    let hit = fixture.request("R", None).join().unwrap().unwrap();
    assert_eq!(hit.cache, CacheOutcome::Hit);
}

const ENTRY: &str = "(symbol \"R_US\"\n\t\t(property \"Reference\" \"R\")\n\t)";

fn stored(name: &str, bytes: &[u8]) -> StoredItem {
    StoredItem {
        lib_ref: LibRef {
            library: "g-res".into(),
            item: name.into(),
        },
        blob_sha256: sha256_hex(bytes),
        format_version: Some("20241209".into()),
    }
}

#[test]
fn stored_symbols_are_drawn_from_their_bytes_and_cached_by_blob() {
    let fixture = fixture(true);
    let item = stored("R_US", ENTRY.as_bytes());
    let load = || Ok(ENTRY.as_bytes().to_vec());
    let first = fixture
        .cache
        .stored_symbol(&fixture.kicad, &item, load, None)
        .unwrap();
    assert_eq!(first.cache, CacheOutcome::Exported);
    assert_eq!(first.library_sha256, item.blob_sha256);
    let entry = first.images[0].file.to_string_lossy().into_owned();
    assert!(
        entry.contains(&format!("sym-blob-{}", item.blob_sha256)),
        "{entry}"
    );
    // A cached preview needs no bytes at all.
    let second = fixture
        .cache
        .stored_symbol(&fixture.kicad, &item, || Err("offline".into()), None)
        .unwrap();
    assert_eq!(second.cache, CacheOutcome::Hit);
    assert_eq!(fixture.cache.export_count(), 1);
}

#[test]
fn a_footprint_entry_keeps_the_file_it_was_drawn_from() {
    let fixture = fixture(true);
    let bytes = b"(footprint \"SOT\" (pad \"1\" smd rect (at 0 0) (size 1 1)))".to_vec();
    let item = StoredItem {
        lib_ref: LibRef {
            library: "g-sot".into(),
            item: "SOT".into(),
        },
        blob_sha256: sha256_hex(&bytes),
        format_version: None,
    };
    let load = || Ok(bytes.clone());
    let first = fixture
        .cache
        .stored_footprint(&fixture.kicad, &item, load, None)
        .unwrap();
    assert_eq!(first.cache, CacheOutcome::Exported);
    let key = first.footprint_key.clone().expect("a key");
    assert_eq!(
        fixture.cache.footprint_source(&key).as_deref(),
        Some(&bytes[..])
    );
    // A cache hit gives the same key, and the bytes without fetching them.
    let second = fixture
        .cache
        .stored_footprint(&fixture.kicad, &item, || Err("offline".into()), None)
        .unwrap();
    assert_eq!(second.cache, CacheOutcome::Hit);
    assert_eq!(second.footprint_key.as_deref(), Some(key.as_str()));
    // Symbols keep nothing of the kind.
    let symbol = fixture.request("R", None).join().unwrap().unwrap();
    assert_eq!(symbol.footprint_key, None);
    // A key names a footprint entry and nothing else.
    let parts: Vec<&str> = key.split('/').collect();
    for bad in [
        String::new(),
        "../../etc".to_owned(),
        format!("{}/..", key),
        format!(
            "{}/{}/{}/{}/../{}",
            parts[0], parts[1], parts[2], parts[3], parts[4]
        ),
        format!("v1/{}/{}/{}/{}", parts[1], parts[2], parts[3], parts[4]),
        format!("{}/{}/{}/sym-x/{}", parts[0], parts[1], parts[2], parts[4]),
        format!("{}/{}/{}/{}/C:\\x", parts[0], parts[1], parts[2], parts[3]),
        key.replace('/', "\\"),
    ] {
        assert_eq!(fixture.cache.footprint_source(&bad), None, "{bad}");
    }
}

#[test]
fn stored_bytes_that_do_not_match_or_cannot_be_drawn_are_refused() {
    let fixture = fixture(true);
    let item = stored("R_US", ENTRY.as_bytes());
    let cache = &fixture.cache;
    // A SHA-256 that is not 64 lowercase hex digits never reaches a cache
    // path, and the bytes are not even fetched.
    for bad in ["../../evil", "ABCDEF", &"A".repeat(64), &"0".repeat(63)] {
        let named_badly = StoredItem {
            blob_sha256: bad.to_owned(),
            ..item.clone()
        };
        let fetch = || -> Result<Vec<u8>, String> { panic!("fetched bytes for {bad}") };
        assert!(matches!(
            cache.stored_symbol(&fixture.kicad, &named_badly, fetch, None),
            Err(PreviewError::StoredBytes(message)) if message.contains("64 lowercase hex digits")
        ));
        assert!(matches!(
            cache.stored_footprint(&fixture.kicad, &named_badly, fetch, None),
            Err(PreviewError::StoredBytes(_))
        ));
    }
    assert!(matches!(
        cache.stored_symbol(&fixture.kicad, &item, || Ok(b"other".to_vec()), None),
        Err(PreviewError::StoredBytes(message)) if message.contains("do not match their SHA-256")
    ));
    assert_eq!(
        cache.stored_symbol(&fixture.kicad, &item, || Err("server down".into()), None),
        Err(PreviewError::StoredBytes("server down".into()))
    );
    let child = "(symbol \"R_Child\"\n\t\t(extends \"R_US\")\n\t)";
    assert!(matches!(
        cache.stored_symbol(&fixture.kicad, &stored("R_Child", child.as_bytes()), || Ok(child.as_bytes().to_vec()), None),
        Err(PreviewError::Unexportable(message)) if message.contains("based on the symbol R_US")
    ));
    // Extra entries may only be the parents the symbol extends: an
    // unrelated symbol beside a derived symbol and its parent is refused,
    // and so is a symbol with an extra one beside it.
    let unrelated = format!(
        "(symbol \"Other\")
{ENTRY}
{child}"
    );
    assert!(matches!(
        cache.stored_symbol(&fixture.kicad, &stored("R_Child", unrelated.as_bytes()), || Ok(unrelated.as_bytes().to_vec()), None),
        Err(PreviewError::Unexportable(message)) if message.contains("not exactly one symbol named R_Child")
    ));
    let beside = format!(
        "{ENTRY}
(symbol \"Other\")"
    );
    assert!(matches!(
        cache.stored_symbol(
            &fixture.kicad,
            &stored("R_US", beside.as_bytes()),
            || Ok(beside.as_bytes().to_vec()),
            None
        ),
        Err(PreviewError::Unexportable(_))
    ));
    // The name must match exactly, case included.
    assert!(matches!(
        cache.stored_symbol(
            &fixture.kicad,
            &stored("r_us", ENTRY.as_bytes()),
            || Ok(ENTRY.as_bytes().to_vec()),
            None
        ),
        Err(PreviewError::Unexportable(_))
    ));
    assert_eq!(cache.export_count(), 0);
}

#[test]
fn stored_items_with_names_that_could_leave_the_staging_folder_are_refused() {
    let fixture = fixture(true);
    let root = fixture._dir.path().to_path_buf();
    let footprint = b"(footprint \"x\")".to_vec();
    for (library, item) in [
        ("g-file", "../../../escaped"),
        ("g-file", r"..\..\..\escaped"),
        ("g-file", r"C:\escaped\x"),
        ("g-file", "/escaped"),
        ("g-file", "a\u{0}b"),
        ("g-file", "a:b"),
        ("g-file", "a\u{1}b"),
        ("../../escaped", "x"),
        ("g-file", ".."),
    ] {
        let item = StoredItem {
            lib_ref: LibRef {
                library: library.into(),
                item: item.into(),
            },
            blob_sha256: sha256_hex(&footprint),
            format_version: Some("20241209".into()),
        };
        let bytes = footprint.clone();
        assert!(
            matches!(
                fixture
                    .cache
                    .stored_footprint(&fixture.kicad, &item, || Ok(bytes), None),
                Err(PreviewError::Unexportable(_))
            ),
            "{library}:{:?}",
            item.lib_ref.item
        );
        let entry = ENTRY.as_bytes().to_vec();
        assert!(matches!(
            fixture
                .cache
                .stored_symbol(&fixture.kicad, &item, || Ok(entry), None),
            Err(PreviewError::Unexportable(_))
        ));
    }
    let escaped: Vec<_> = walk(&root)
        .into_iter()
        .filter(|path| path.to_string_lossy().contains("escaped"))
        .collect();
    assert!(escaped.is_empty(), "{escaped:?}");
    assert!(!Path::new(r"C:\escaped").exists());
    assert_eq!(fixture.cache.export_count(), 0);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        found.push(path.clone());
        if path.is_dir() {
            found.extend(walk(&path));
        }
    }
    found
}

// ---- 3D views ----------------------------------------------------------------

fn board_export<'a>(board: &'a str, models: &'a [PathBuf]) -> BoardExport<'a> {
    BoardExport {
        name: "R_0603",
        board: board.as_bytes(),
        models,
    }
}

#[test]
fn a_3d_view_is_exported_once_and_its_key_changes_with_a_model_file() {
    let fixture = fixture(true);
    let model = fixture.library.with_file_name("R.step");
    fs::write(&model, "ISO-10303-21; one").unwrap();
    let models = [model.clone()];
    let first = fixture
        .cache
        .board_glb(&fixture.kicad, &board_export("(kicad_pcb)", &models), None)
        .unwrap();
    assert_eq!(first.cache, CacheOutcome::Exported);
    assert_eq!(first.messages, "Binary GLTF file created.");
    assert!(
        first
            .key
            .starts_with("v2/kicad-10.0.4/theme_builtin_default/glb-"),
        "{}",
        first.key
    );
    let bytes = fixture.cache.glb_bytes(&first.key).unwrap();
    assert!(bytes.starts_with(b"glTF"));
    assert_eq!(first.size, bytes.len() as u64);

    // The same board and model: served from the cache, kicad-cli not run.
    let again = fixture
        .cache
        .board_glb(&fixture.kicad, &board_export("(kicad_pcb)", &models), None)
        .unwrap();
    assert_eq!((again.cache, &again.key), (CacheOutcome::Hit, &first.key));
    assert_eq!(fixture.cache.export_count(), 1);

    // The model file changes: a new 3D view. So does the board.
    fs::write(&model, "ISO-10303-21; two, longer").unwrap();
    let changed = fixture
        .cache
        .board_glb(&fixture.kicad, &board_export("(kicad_pcb)", &models), None)
        .unwrap();
    assert_ne!(changed.key, first.key);
    let other_board = fixture
        .cache
        .board_glb(&fixture.kicad, &board_export("(kicad_pcb )", &models), None)
        .unwrap();
    assert_ne!(other_board.key, changed.key);
    assert_eq!(fixture.cache.export_count(), 3);

    // A key names only a 3D view of this cache.
    let source_like = first.key.replace("/glb-", "/fp-");
    for key in [source_like.as_str(), "v2/../../x/glb-a/b", "glb-a", ""] {
        assert_eq!(fixture.cache.glb_bytes(key), None, "{key}");
    }
}

#[test]
fn a_3d_export_that_writes_nothing_fails() {
    let dir = tempfile::tempdir().unwrap();
    let runner: Runner = Arc::new(|_, _, _| {
        Ok(CliOutput {
            code: Some(0),
            stdout: "Could not add 3D model for REF**.".into(),
            stderr: String::new(),
            timed_out: false,
        })
    });
    let cache = PreviewCache::with_runner(dir.path().join("cache"), runner);
    let kicad = fixture(true).kicad;
    match cache.board_glb(&kicad, &board_export("(kicad_pcb)", &[]), None) {
        Err(PreviewError::ExportFailed {
            command,
            cli_message,
        }) => {
            assert!(command.starts_with("kicad-cli pcb export glb --include-pads --include-silkscreen --include-soldermask --output "), "{command}");
            assert_eq!(cli_message, "Could not add 3D model for REF**.");
        }
        other => panic!("{other:?}"),
    }
    // A missing model file is an error, not an empty hash.
    let missing = [dir.path().join("gone.step")];
    assert!(
        cache
            .board_glb(&kicad, &board_export("(kicad_pcb)", &missing), None)
            .is_err()
    );
}

#[test]
fn exports_of_3d_views_wait_in_the_same_queue_as_previews() {
    let fixture = fixture(false);
    let previews: Vec<_> = ["A", "B"]
        .iter()
        .map(|name| fixture.request(name, None))
        .collect();
    fixture.fake.wait_until_started(2);
    let board = {
        let (cache, kicad) = (fixture.cache.clone(), fixture.kicad.clone());
        thread::spawn(move || cache.board_glb(&kicad, &board_export("(kicad_pcb)", &[]), None))
    };
    fixture.wait_until_joined(3);
    // Two previews hold both turns, so the 3D export has not started.
    thread::sleep(Duration::from_millis(50));
    assert_eq!(fixture.fake.started().len(), 2);
    fixture.fake.open_gate();
    for preview in previews {
        preview.join().unwrap().unwrap();
    }
    board.join().unwrap().unwrap();
    assert_eq!(fixture.fake.peak.load(Ordering::SeqCst), MAX_RUNNING);
    assert_eq!(
        fixture.fake.started().last().map(String::as_str),
        Some("board")
    );
}

#[test]
fn a_3d_export_only_an_older_selection_wants_is_dropped() {
    let fixture = fixture(false);
    let busy: Vec<_> = ["A", "B"]
        .iter()
        .map(|name| fixture.request(name, None))
        .collect();
    fixture.fake.wait_until_started(2);
    let board = {
        let (cache, kicad) = (fixture.cache.clone(), fixture.kicad.clone());
        thread::spawn(move || cache.board_glb(&kicad, &board_export("(kicad_pcb)", &[]), Some(1)))
    };
    fixture.wait_until_joined(3);
    let newer = fixture.request("C", Some(2));
    fixture.fake.open_gate();
    assert_eq!(board.join().unwrap(), Err(PreviewError::Replaced));
    newer.join().unwrap().unwrap();
    for preview in busy {
        preview.join().unwrap().unwrap();
    }
    assert!(!fixture.fake.started().contains(&"board".to_owned()));
}

#[test]
fn model_copies_are_checked_and_stored_once() {
    let fixture = fixture(true);
    let bytes = b"ISO-10303-21; stored".to_vec();
    let sha = sha256_hex(&bytes);
    let path = fixture
        .cache
        .model_copy(&fixture.kicad, &sha, "R.STEP", || Ok(bytes.clone()))
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(path.file_name().unwrap(), "model.step");
    // Stored already: the bytes are not loaded again.
    let again = fixture
        .cache
        .model_copy(&fixture.kicad, &sha, "R.STEP", || panic!("loaded again"))
        .unwrap();
    assert_eq!(again, path);
    // The same bytes under another name and type get their own copy.
    let stp = fixture
        .cache
        .model_copy(&fixture.kicad, &sha, "R.stp", || Ok(bytes.clone()))
        .unwrap();
    assert_eq!(stp.file_name().unwrap(), "model.stp");
    assert_ne!(stp.parent(), path.parent());
    // Bytes that do not match their SHA-256 are refused.
    let other = sha256_hex(b"other");
    let refused = fixture
        .cache
        .model_copy(&fixture.kicad, &other, "R.step", || Ok(bytes.clone()))
        .unwrap_err();
    assert!(
        matches!(refused, PreviewError::StoredBytes(ref why) if why.contains("do not match")),
        "{refused:?}"
    );
    assert!(
        fixture
            .cache
            .model_copy(&fixture.kicad, "ABC", "R.step", || Ok(bytes.clone()))
            .is_err()
    );
    for name in ["R.kicad_mod", "id_rsa", "R"] {
        assert!(
            fixture
                .cache
                .model_copy(&fixture.kicad, &sha, name, || panic!("{name} loaded"))
                .is_err(),
            "{name}"
        );
    }
}

/// kicad-cli 10.0.4 ends with code 2 when a STEP file cannot be read, but
/// still writes the board without that model.
#[test]
fn a_3d_export_that_could_not_read_a_model_still_counts() {
    let complaints = "**** ERR StepFile : Undefined Parsing: Line 4: Incorrect syntax ****\nreadSTEP() failed on filename 'C:\\m\\bad.step'.\nNo model for filename 'C:\\m\\bad.step'.\nBinary GLTF file 'b.glb' created.\n\nExport time 0.014 s";
    assert!(only_model_complaints(complaints));
    assert!(!only_model_complaints("Binary GLTF file 'b.glb' created."));
    assert!(!only_model_complaints(
        "No model for filename 'x'.\nFailed to load board"
    ));
    let run = |code: i32, stdout: &'static str| {
        let dir = tempfile::tempdir().unwrap();
        let runner: Runner = Arc::new(move |_, args, _| {
            let at = args.iter().position(|arg| arg == "--output").unwrap();
            fs::write(&args[at + 1], fake_glb()).unwrap();
            Ok(CliOutput {
                code: Some(code),
                stdout: stdout.into(),
                stderr: String::new(),
                timed_out: false,
            })
        });
        let cache = PreviewCache::with_runner(dir.path().join("cache"), runner);
        let kicad = fixture(true).kicad;
        let result = cache.board_glb(&kicad, &board_export("(kicad_pcb)", &[]), None);
        (result, dir)
    };
    let (read, _dir) = run(2, complaints);
    assert_eq!(
        read.unwrap().messages,
        complaints,
        "the messages are kept for the 3D view"
    );
    let (other, _dir) = run(2, "Failed to load board");
    assert!(
        matches!(other, Err(PreviewError::ExportFailed { .. })),
        "{other:?}"
    );
    let (crash, _dir) = run(3, complaints);
    assert!(
        matches!(crash, Err(PreviewError::ExportFailed { .. })),
        "{crash:?}"
    );
}

/// A GLB header whose length is its own, and nothing else.
fn fake_glb() -> Vec<u8> {
    let mut bytes = b"glTF".to_vec();
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&20u32.to_le_bytes());
    bytes.extend_from_slice(b"JSON a 3D");
    bytes.truncate(20);
    bytes
}

#[test]
fn only_a_whole_glb_counts() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("b.glb");
    fs::write(&file, fake_glb()).unwrap();
    assert!(whole_glb(&file));
    // Cut off, or with more than its header says: not whole.
    fs::write(&file, &fake_glb()[..16]).unwrap();
    assert!(!whole_glb(&file));
    let mut longer = fake_glb();
    longer.extend_from_slice(b"more");
    fs::write(&file, longer).unwrap();
    assert!(!whole_glb(&file));
    fs::write(&file, b"glTF").unwrap();
    assert!(!whole_glb(&file));
    assert!(!whole_glb(&dir.path().join("none.glb")));
}
