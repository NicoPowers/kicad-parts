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
        let name = value("--symbol").or_else(|| value("--footprint")).unwrap();
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
    let older_kicad = fixture.cache.root().join("v1").join("kicad-9.0.0");
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
