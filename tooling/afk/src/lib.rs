mod lock;
mod policy;

pub use lock::{VersionsLock, validate_digest};
pub use policy::{
    ComposePolicyReport, PolicyExpectation, expected_service_resource_bounds,
    expected_service_tmpfs_bounds, validate_compose_policy,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub type LabResult<T> = Result<T, LabError>;

#[derive(Debug)]
pub struct LabError(pub String);

impl std::fmt::Display for LabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LabError {}

impl From<std::io::Error> for LabError {
    fn from(value: std::io::Error) -> Self {
        Self(value.to_string())
    }
}

impl From<serde_json::Error> for LabError {
    fn from(value: serde_json::Error) -> Self {
        Self(value.to_string())
    }
}

impl From<toml::de::Error> for LabError {
    fn from(value: toml::de::Error) -> Self {
        Self(value.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestCredentials {
    pub minio_user: String,
    pub minio_password: String,
}

impl TestCredentials {
    pub fn secret_values(&self) -> [&str; 2] {
        [&self.minio_user, &self.minio_password]
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunImages {
    pub pocketbase: String,
    pub minio: String,
    pub test_runner: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RunMetadata {
    schema: u32,
    run_id: String,
    compose_project: String,
    repository_fingerprint: String,
    images: RunImages,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunPlan {
    pub run_id: String,
    pub compose_project: String,
    pub repository: PathBuf,
    pub run_root: PathBuf,
    pub state_dir: PathBuf,
    pub artifact_dir: PathBuf,
    pub images: RunImages,
    pub credentials: TestCredentials,
}

impl RunPlan {
    pub fn create(
        repository: &Path,
        requested_id: Option<&str>,
        lock: &VersionsLock,
    ) -> LabResult<Self> {
        let repository = repository
            .canonicalize()
            .map_err(|error| LabError(format!("cannot resolve repository root: {error}")))?;
        let run_id = match requested_id {
            Some(value) => sanitize_run_id(value)?,
            None => unique_run_id(&repository)?,
        };
        let compose_project = format!("kp-afk-{run_id}");
        let run_root = repository.join(".afk").join("runs").join(&run_id);
        let fingerprint = &digest(compose_path(&repository).as_bytes())[..12];
        let image_prefix = format!("kicad-parts-afk/{fingerprint}/{run_id}");
        let images = RunImages {
            pocketbase: format!(
                "{image_prefix}/pocketbase:{}",
                lock.components.pocketbase.version
            ),
            minio: format!(
                "{image_prefix}/minio:{}",
                lock.components.minio.short_commit
            ),
            test_runner: format!(
                "{image_prefix}/test-runner:rust-{}-node-{}",
                lock.toolchain.rust, lock.toolchain.node
            ),
        };
        let credentials = TestCredentials {
            minio_user: format!("afk-{}", &digest(format!("{run_id}:user").as_bytes())[..12]),
            minio_password: digest(format!("{run_id}:password:{}", entropy_nanos()).as_bytes()),
        };
        Ok(Self {
            run_id,
            compose_project,
            repository: repository.clone(),
            state_dir: run_root.join("state"),
            artifact_dir: run_root.join("artifacts"),
            run_root,
            images,
            credentials,
        })
    }

    pub fn open_existing(repository: &Path, run_id: &str, lock: &VersionsLock) -> LabResult<Self> {
        let plan = Self::create(repository, Some(run_id), lock)?;
        let metadata_path = plan.state_dir.join("run.json");
        let metadata: RunMetadata =
            serde_json::from_str(&fs::read_to_string(&metadata_path).map_err(|error| {
                LabError(format!(
                    "cannot read scoped run metadata `{}`: {error}",
                    metadata_path.display()
                ))
            })?)?;
        let wanted = plan.metadata();
        if metadata != wanted {
            return Err(LabError(
                "scoped run metadata does not match this repository/lock".into(),
            ));
        }
        let canonical_root = plan.run_root.canonicalize()?;
        let canonical_runs = plan.repository.join(".afk/runs").canonicalize()?;
        if canonical_root.parent() != Some(canonical_runs.as_path())
            || has_link_or_reparse(&canonical_root)?
        {
            return Err(LabError(
                "refusing non-canonical or linked AFK run root".into(),
            ));
        }
        Ok(plan)
    }

    pub fn prepare(&self) -> LabResult<()> {
        fs::create_dir_all(self.repository.join(".afk/runs"))?;
        fs::create_dir(&self.run_root).map_err(|error| {
            LabError(format!(
                "run `{}` already exists or cannot be reserved atomically: {error}",
                self.run_id
            ))
        })?;
        let result = (|| {
            fs::create_dir(&self.state_dir)?;
            fs::create_dir(&self.artifact_dir)?;
            write_json_new(&self.state_dir.join("run.json"), &self.metadata())?;
            fs::write(
                self.state_dir.join("run-canary"),
                digest(format!("{}:run-canary", self.run_id).as_bytes()),
            )?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&self.run_root);
        }
        result
    }

    fn metadata(&self) -> RunMetadata {
        RunMetadata {
            schema: 1,
            run_id: self.run_id.clone(),
            compose_project: self.compose_project.clone(),
            repository_fingerprint: digest(compose_path(&self.repository).as_bytes())[..12].into(),
            images: self.images.clone(),
        }
    }

    pub fn run_canary_hash(&self) -> LabResult<String> {
        let path = self.state_dir.join("run-canary");
        let bytes = fs::read(&path).map_err(|error| {
            LabError(format!(
                "cannot read scoped run canary `{}`: {error}",
                path.display()
            ))
        })?;
        Ok(digest(&bytes))
    }

    pub fn remove_state(&self) -> LabResult<()> {
        let canonical_runs = self.repository.join(".afk/runs").canonicalize()?;
        let canonical_root = self.run_root.canonicalize()?;
        if canonical_root.parent() != Some(canonical_runs.as_path())
            || self.state_dir.parent() != Some(self.run_root.as_path())
            || has_link_or_reparse(&canonical_root)?
        {
            return Err(LabError(
                "refusing to remove unscoped or linked AFK state".into(),
            ));
        }
        if self.state_dir.exists() {
            fs::remove_dir_all(&self.state_dir)?;
        }
        Ok(())
    }

    pub fn cleanup_command(&self) -> String {
        format!("cargo xtask cleanup-afk --run-id {}", self.run_id)
    }

    pub fn compose_environment(&self, lock: &VersionsLock) -> BTreeMap<String, String> {
        let uid = lock.toolchain.runner_uid.to_string();
        let gid = lock.toolchain.runner_gid.to_string();
        BTreeMap::from([
            ("AFK_COMPOSE_PROJECT".into(), self.compose_project.clone()),
            ("AFK_MINIO_USER".into(), self.credentials.minio_user.clone()),
            (
                "AFK_MINIO_PASSWORD".into(),
                self.credentials.minio_password.clone(),
            ),
            (
                "AFK_POCKETBASE_IMAGE".into(),
                self.images.pocketbase.clone(),
            ),
            ("AFK_MINIO_IMAGE".into(), self.images.minio.clone()),
            (
                "AFK_TEST_RUNNER_IMAGE".into(),
                self.images.test_runner.clone(),
            ),
            (
                "AFK_BUSYBOX_IMAGE".into(),
                lock.images.busybox.reference.clone(),
            ),
            ("AFK_RUST_IMAGE".into(), lock.images.rust.reference.clone()),
            ("AFK_RUNNER_UID".into(), uid),
            ("AFK_RUNNER_GID".into(), gid),
            (
                "AFK_POCKETBASE_VERSION".into(),
                lock.components.pocketbase.version.clone(),
            ),
            (
                "AFK_POCKETBASE_URL".into(),
                lock.components.pocketbase.url.clone(),
            ),
            (
                "AFK_POCKETBASE_SHA256".into(),
                lock.components.pocketbase.sha256.clone(),
            ),
            (
                "AFK_MINIO_COMMIT".into(),
                lock.components.minio.commit.clone(),
            ),
            (
                "AFK_MINIO_SHORT_COMMIT".into(),
                lock.components.minio.short_commit.clone(),
            ),
            (
                "AFK_MINIO_VERSION".into(),
                lock.components.minio.version.clone(),
            ),
            (
                "AFK_MINIO_BUILD_VERSION".into(),
                lock.components.minio.build_version.clone(),
            ),
            (
                "AFK_MINIO_COPYRIGHT_YEAR".into(),
                lock.components.minio.copyright_year.clone(),
            ),
            (
                "AFK_MINIO_SOURCE_URL".into(),
                lock.components.minio.source_url.clone(),
            ),
            (
                "AFK_MINIO_SOURCE_SHA256".into(),
                lock.components.minio.source_sha256.clone(),
            ),
            ("AFK_GO_URL".into(), lock.toolchain.go_url.clone()),
            (
                "AFK_GO_SHA256".into(),
                lock.toolchain.go_linux_x64_sha256.clone(),
            ),
            (
                "AFK_RUST_TOOLCHAIN_DIR".into(),
                lock.toolchain.rust_toolchain_dir.clone(),
            ),
            (
                "AFK_DEBIAN_SNAPSHOT".into(),
                lock.toolchain.debian_snapshot.clone(),
            ),
            ("AFK_NODE_VERSION".into(), lock.toolchain.node.clone()),
            ("AFK_NODE_URL".into(), lock.toolchain.node_url.clone()),
            (
                "AFK_NODE_SHA256".into(),
                lock.toolchain.node_linux_x64_sha256.clone(),
            ),
            ("AFK_TAURI_VERSION".into(), lock.toolchain.tauri_cli.clone()),
            ("AFK_TAURI_URL".into(), lock.toolchain.tauri_cli_url.clone()),
            (
                "AFK_TAURI_SHA512".into(),
                lock.toolchain.tauri_cli_sha512.clone(),
            ),
            (
                "AFK_TAURI_LINUX_X64_GNU_URL".into(),
                lock.toolchain.tauri_cli_linux_x64_gnu_url.clone(),
            ),
            (
                "AFK_TAURI_LINUX_X64_GNU_SHA512".into(),
                lock.toolchain.tauri_cli_linux_x64_gnu_sha512.clone(),
            ),
        ])
    }

    pub fn policy_expectation(&self, lock: &VersionsLock) -> PolicyExpectation {
        let environment = self.compose_environment(lock);
        let args = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(key, env_key)| ((*key).into(), environment[*env_key].clone()))
                .collect::<BTreeMap<String, String>>()
        };
        PolicyExpectation {
            project: self.compose_project.clone(),
            user: format!(
                "{}:{}",
                lock.toolchain.runner_uid, lock.toolchain.runner_gid
            ),
            images: BTreeMap::from([
                ("pocketbase".into(), self.images.pocketbase.clone()),
                ("minio".into(), self.images.minio.clone()),
                ("test-runner".into(), self.images.test_runner.clone()),
            ]),
            build_contexts: BTreeMap::from([
                (
                    "pocketbase".into(),
                    self.repository.join("infra/docker/pocketbase"),
                ),
                ("minio".into(), self.repository.join("infra/docker/minio")),
                (
                    "test-runner".into(),
                    self.repository.join("infra/docker/test-runner"),
                ),
            ]),
            build_args: BTreeMap::from([
                (
                    "pocketbase".into(),
                    args(&[
                        ("AFK_GID", "AFK_RUNNER_GID"),
                        ("AFK_UID", "AFK_RUNNER_UID"),
                        ("BUSYBOX_IMAGE", "AFK_BUSYBOX_IMAGE"),
                        ("POCKETBASE_SHA256", "AFK_POCKETBASE_SHA256"),
                        ("POCKETBASE_URL", "AFK_POCKETBASE_URL"),
                        ("POCKETBASE_VERSION", "AFK_POCKETBASE_VERSION"),
                    ]),
                ),
                (
                    "minio".into(),
                    args(&[
                        ("AFK_GID", "AFK_RUNNER_GID"),
                        ("AFK_UID", "AFK_RUNNER_UID"),
                        ("BUSYBOX_IMAGE", "AFK_BUSYBOX_IMAGE"),
                        ("GO_SHA256", "AFK_GO_SHA256"),
                        ("GO_URL", "AFK_GO_URL"),
                        ("MINIO_BUILD_VERSION", "AFK_MINIO_BUILD_VERSION"),
                        ("MINIO_COPYRIGHT_YEAR", "AFK_MINIO_COPYRIGHT_YEAR"),
                        ("MINIO_COMMIT", "AFK_MINIO_COMMIT"),
                        ("MINIO_SHORT_COMMIT", "AFK_MINIO_SHORT_COMMIT"),
                        ("MINIO_SOURCE_SHA256", "AFK_MINIO_SOURCE_SHA256"),
                        ("MINIO_SOURCE_URL", "AFK_MINIO_SOURCE_URL"),
                        ("MINIO_VERSION", "AFK_MINIO_VERSION"),
                        ("RUST_IMAGE", "AFK_RUST_IMAGE"),
                    ]),
                ),
                (
                    "test-runner".into(),
                    args(&[
                        ("AFK_GID", "AFK_RUNNER_GID"),
                        ("AFK_UID", "AFK_RUNNER_UID"),
                        ("DEBIAN_SNAPSHOT", "AFK_DEBIAN_SNAPSHOT"),
                        ("NODE_SHA256", "AFK_NODE_SHA256"),
                        ("NODE_URL", "AFK_NODE_URL"),
                        ("NODE_VERSION", "AFK_NODE_VERSION"),
                        ("RUST_IMAGE", "AFK_RUST_IMAGE"),
                        ("RUST_TOOLCHAIN_DIR", "AFK_RUST_TOOLCHAIN_DIR"),
                        ("TAURI_CLI_SHA512", "AFK_TAURI_SHA512"),
                        ("TAURI_CLI_URL", "AFK_TAURI_URL"),
                        ("TAURI_CLI_VERSION", "AFK_TAURI_VERSION"),
                        (
                            "TAURI_CLI_LINUX_X64_GNU_SHA512",
                            "AFK_TAURI_LINUX_X64_GNU_SHA512",
                        ),
                        (
                            "TAURI_CLI_LINUX_X64_GNU_URL",
                            "AFK_TAURI_LINUX_X64_GNU_URL",
                        ),
                    ]),
                ),
            ]),
            minio_user: self.credentials.minio_user.clone(),
            minio_password: self.credentials.minio_password.clone(),
        }
    }
}

fn sanitize_run_id(value: &str) -> LabResult<String> {
    let value = value.to_ascii_lowercase();
    if value.len() < 6
        || value.len() > 48
        || !value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        || value.starts_with('-')
        || value.ends_with('-')
    {
        return Err(LabError(
            "run id must be 6-48 lowercase ASCII letters, digits, or interior hyphens".into(),
        ));
    }
    Ok(value)
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_run_id(repository: &Path) -> LabResult<String> {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| LabError(format!("system clock is before Unix epoch: {error}")))?
        .as_micros();
    let repo = digest(compose_path(repository).as_bytes());
    let counter = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(format!(
        "{}-{micros:x}-{:x}-{counter:x}",
        &repo[..8],
        std::process::id()
    ))
}

fn entropy_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

pub fn compose_path(path: &Path) -> String {
    let rendered = path.to_string_lossy().replace('\\', "/");
    rendered
        .strip_prefix("//?/")
        .unwrap_or(&rendered)
        .to_owned()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposeCommand {
    pub program: String,
    pub prefix_args: Vec<String>,
}

impl ComposeCommand {
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.prefix_args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub fn choose_compose(plugin_ok: bool, standalone_ok: bool) -> LabResult<ComposeCommand> {
    if plugin_ok {
        return Ok(ComposeCommand {
            program: "docker".into(),
            prefix_args: vec!["compose".into()],
        });
    }
    if standalone_ok {
        return Ok(ComposeCommand {
            program: "docker-compose".into(),
            prefix_args: Vec::new(),
        });
    }
    Err(LabError(
        "neither `docker compose` nor `docker-compose` is available".into(),
    ))
}

pub fn redact(input: &str, secrets: &[&str]) -> String {
    let mut values = secrets
        .iter()
        .filter(|value| !value.is_empty())
        .copied()
        .collect::<Vec<_>>();
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values.dedup();
    values.into_iter().fold(input.to_owned(), |text, secret| {
        text.replace(secret, "[REDACTED]")
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupAction {
    TearDown,
    PreserveRun,
}

pub fn cleanup_action(success: bool, preserve_on_failure: bool) -> CleanupAction {
    if !success && preserve_on_failure {
        CleanupAction::PreserveRun
    } else {
        CleanupAction::TearDown
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureStage {
    MissingCompose,
    AfterPlan,
    AfterStart,
    HealthMismatch,
    VersionMismatch,
    EvidenceWrite,
    TeardownFailure,
    CommandTimeout,
}

impl FailureStage {
    pub fn parse(value: &str) -> LabResult<Self> {
        match value {
            "missing-compose" => Ok(Self::MissingCompose),
            "after-plan" => Ok(Self::AfterPlan),
            "after-start" => Ok(Self::AfterStart),
            "health-mismatch" => Ok(Self::HealthMismatch),
            "version-mismatch" => Ok(Self::VersionMismatch),
            "evidence-write" => Ok(Self::EvidenceWrite),
            "teardown-failure" => Ok(Self::TeardownFailure),
            "command-timeout" => Ok(Self::CommandTimeout),
            _ => Err(LabError(format!(
                "unknown failure injection stage `{value}`"
            ))),
        }
    }

    pub fn compatible_with_static(self) -> bool {
        matches!(
            self,
            Self::MissingCompose | Self::AfterPlan | Self::EvidenceWrite
        )
    }
}

pub fn protected_sentinels(repository: &Path) -> LabResult<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for relative in [
        "database",
        "symbols",
        "footprints",
        "3d-models",
        "fixtures/import",
    ] {
        let root = repository.join(relative);
        result.insert(relative.into(), tree_digest(repository, &root)?);
    }
    Ok(result)
}

fn tree_digest(repository: &Path, root: &Path) -> LabResult<String> {
    let mut files = Vec::new();
    collect_regular_files(repository, root, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for path in files {
        let relative = path
            .strip_prefix(repository)
            .map_err(|_| LabError("sentinel path escaped repository".into()))?;
        let bytes = fs::read(&path)?;
        hasher.update(compose_path(relative));
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_regular_files(root: &Path, path: &Path, out: &mut Vec<PathBuf>) -> LabResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        LabError(format!(
            "cannot inspect protected input `{}`: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(LabError(format!(
            "protected input contains link/reparse point `{}`",
            path.display()
        )));
    }
    if metadata.is_file() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(LabError(format!(
            "protected input is not a file/directory `{}`",
            path.display()
        )));
    }
    let mut children = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        let child = entry.path();
        if !child.starts_with(root) {
            return Err(LabError("protected input escaped repository".into()));
        }
        collect_regular_files(root, &child, out)?;
    }
    Ok(())
}

pub fn host_secret_path_exists(repository: &Path) -> LabResult<bool> {
    match fs::symlink_metadata(repository.join("secrets.env")) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(LabError(format!(
            "cannot inspect host secret path metadata: {error}"
        ))),
    }
}

fn has_link_or_reparse(path: &Path) -> LabResult<bool> {
    let mut cursor = Some(path);
    while let Some(candidate) = cursor {
        let metadata = fs::symlink_metadata(candidate)?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Ok(true);
        }
        cursor = candidate.parent();
    }
    Ok(false)
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LaneStatus {
    Passed,
    Failed,
    NotRun,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LaneResult {
    pub status: LaneStatus,
    pub detail: String,
    pub log: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvidenceManifest {
    pub schema: u32,
    pub run_id: String,
    pub overall: LaneStatus,
    pub lanes: BTreeMap<String, LaneResult>,
    pub artifacts: BTreeMap<String, ArtifactIntegrity>,
    pub unhashed_self: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactIntegrity {
    pub bytes: u64,
    pub sha256: String,
}

pub fn initial_lanes() -> BTreeMap<String, LaneResult> {
    [
        ("static-policy", "not reached"),
        ("foundation-stack", "not reached"),
        ("test-runner", "not reached"),
        ("orchestration", "not reached"),
    ]
    .into_iter()
    .map(|(name, detail)| {
        (
            name.into(),
            LaneResult {
                status: LaneStatus::NotRun,
                detail: detail.into(),
                log: None,
            },
        )
    })
    .collect()
}

pub fn enumerate_artifacts(directory: &Path) -> LabResult<BTreeMap<String, ArtifactIntegrity>> {
    let mut artifacts = BTreeMap::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink()
            || is_reparse_point(&metadata)
            || !metadata.file_type().is_file()
        {
            return Err(LabError(format!(
                "artifact entry is not a non-link regular file: `{}`",
                path.display()
            )));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| LabError("artifact filename is not valid Unicode".into()))?;
        let integrity = artifact_integrity(&path)?;
        if artifacts.insert(name.clone(), integrity).is_some() {
            return Err(LabError(format!(
                "duplicate artifact path after normalization: `{name}`"
            )));
        }
    }
    Ok(artifacts)
}

pub fn artifact_integrity_for_bytes(bytes: &[u8]) -> ArtifactIntegrity {
    ArtifactIntegrity {
        bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
    }
}

fn artifact_integrity(path: &Path) -> LabResult<ArtifactIntegrity> {
    let mut file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(LabError(format!(
            "opened artifact is not a regular file: `{}`",
            path.display()
        )));
    }
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes = bytes
            .checked_add(read as u64)
            .ok_or_else(|| LabError("artifact byte length overflowed".into()))?;
    }
    if bytes != metadata.len() || file.metadata()?.len() != bytes {
        return Err(LabError(format!(
            "artifact changed while hashing: `{}`",
            path.display()
        )));
    }
    Ok(ArtifactIntegrity {
        bytes,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

pub fn verify_artifact_manifest(
    directory: &Path,
    declared: &BTreeMap<String, ArtifactIntegrity>,
) -> LabResult<()> {
    let mut actual = enumerate_artifacts(directory)?;
    if actual.remove("evidence.json").is_none() {
        return Err(LabError(
            "final artifact directory lacks its self-excluded evidence.json".into(),
        ));
    }
    if &actual != declared {
        return Err(LabError(format!(
            "artifact declarations differ from finalized files: actual={actual:?}, declared={declared:?}"
        )));
    }
    Ok(())
}

pub fn write_json_new(path: &Path, value: &impl Serialize) -> LabResult<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            LabError(format!(
                "cannot create `{}` atomically: {error}",
                path.display()
            ))
        })?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

pub fn service_is_healthy(command_succeeded: bool, state: &str) -> LabResult<()> {
    if !command_succeeded {
        return Err(LabError("service health command failed".into()));
    }
    let state = state.to_ascii_lowercase();
    if state.contains("unhealthy")
        || state.contains("exited")
        || state.contains("dead")
        || !state.contains("healthy")
    {
        return Err(LabError(format!(
            "service health state is not proven healthy: {state}"
        )));
    }
    Ok(())
}

pub fn validate_runtime_resource_output(output: &str, service: &str) -> LabResult<()> {
    reject_runtime_state_errors(output)?;
    let (memory, pids) = expected_service_resource_bounds(service)?;
    for marker in [
        format!("AFK_CGROUP_MEMORY={memory}"),
        format!("AFK_CGROUP_PIDS={pids}"),
    ] {
        if output.lines().filter(|line| *line == marker).count() != 1 {
            return Err(LabError(format!(
                "service `{service}` did not prove exact runtime limit `{marker}`"
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeIsolationEvidence {
    pub service: String,
    pub uid: u32,
    pub gid: u32,
    pub memory_bytes: u64,
    pub pids: u64,
    pub tmpfs: BTreeMap<String, RuntimeTmpfsEvidence>,
    pub writable_targets: BTreeMap<String, RuntimeWritableEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeTmpfsEvidence {
    pub owner_uid: u32,
    pub owner_gid: u32,
    pub mode: u64,
    pub size_bytes: u64,
    pub mount_options_verified: bool,
    pub mode_verified_by_stat: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeWritableEvidence {
    pub owner_uid: u32,
    pub owner_gid: u32,
    pub mode: u64,
    pub create_read_delete: bool,
}

pub fn expected_service_writable_targets(service: &str) -> LabResult<BTreeSet<String>> {
    let mut targets = expected_service_tmpfs_bounds(service)?
        .into_keys()
        .collect::<BTreeSet<_>>();
    for target in [
        "/afk/xdg/cache",
        "/afk/xdg/config",
        "/afk/xdg/data",
        "/afk/xdg/state",
    ] {
        targets.insert(target.into());
    }
    Ok(targets)
}

pub fn validate_runtime_isolation_output(
    output: &str,
    service: &str,
    lock: &VersionsLock,
) -> LabResult<RuntimeIsolationEvidence> {
    reject_runtime_state_errors(output)?;
    let uid = lock.toolchain.runner_uid;
    let gid = lock.toolchain.runner_gid;
    let (memory_bytes, pids) = expected_service_resource_bounds(service)?;
    let tmpfs_bounds = expected_service_tmpfs_bounds(service)?;
    let mut expected_markers = BTreeSet::from([
        format!("AFK_ID={uid}:{gid}"),
        format!("AFK_CGROUP_MEMORY={memory_bytes}"),
        format!("AFK_CGROUP_PIDS={pids}"),
    ]);
    let mut tmpfs = BTreeMap::new();
    for (target, (size_bytes, mode)) in &tmpfs_bounds {
        expected_markers.insert(format!(
            "AFK_TMPFS={target}|owner=0:0|mode={mode:o}|size_bytes={size_bytes}|options=rw,size|mode_source=stat"
        ));
        tmpfs.insert(
            target.clone(),
            RuntimeTmpfsEvidence {
                owner_uid: 0,
                owner_gid: 0,
                mode: *mode,
                size_bytes: *size_bytes,
                mount_options_verified: true,
                mode_verified_by_stat: true,
            },
        );
    }
    let mut writable_targets = BTreeMap::new();
    for target in expected_service_writable_targets(service)? {
        let is_mount_root = tmpfs_bounds.contains_key(&target);
        let (owner_uid, owner_gid, mode) = if is_mount_root {
            (0, 0, 0o1777)
        } else {
            (uid, gid, 0o700)
        };
        expected_markers.insert(format!(
            "AFK_WRITABLE={target}|owner={owner_uid}:{owner_gid}|mode={mode:o}|canary=create-read-delete"
        ));
        writable_targets.insert(
            target,
            RuntimeWritableEvidence {
                owner_uid,
                owner_gid,
                mode,
                create_read_delete: true,
            },
        );
    }
    let actual_lines = output
        .lines()
        .filter(|line| line.starts_with("AFK_"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let actual = actual_lines.iter().cloned().collect::<BTreeSet<_>>();
    if actual_lines.len() != actual.len() || actual != expected_markers {
        return Err(LabError(format!(
            "service `{service}` runtime isolation markers differ: actual={actual:?}, expected={expected_markers:?}"
        )));
    }
    Ok(RuntimeIsolationEvidence {
        service: service.into(),
        uid,
        gid,
        memory_bytes,
        pids,
        tmpfs,
        writable_targets,
    })
}

pub fn reject_runtime_state_errors(output: &str) -> LabResult<()> {
    let lower = output.to_ascii_lowercase();
    for forbidden in [
        "permission denied",
        "operation not permitted",
        "read-only file system",
        "failed to create",
        "cannot create",
        "could not create",
        "failed to write",
        "unable to write",
        "configuration error",
        "config error",
    ] {
        if lower.contains(forbidden) {
            return Err(LabError(format!(
                "runtime output contains state/configuration failure `{forbidden}`"
            )));
        }
    }
    Ok(())
}

pub fn validate_pocketbase_output(output: &str, lock: &VersionsLock) -> LabResult<()> {
    exact_trimmed(
        output,
        &lock.components.pocketbase.expected_output,
        "PocketBase version",
    )
}

pub fn validate_minio_output(output: &str, lock: &VersionsLock) -> LabResult<()> {
    reject_runtime_state_errors(output)?;
    for expected in [
        format!(
            "minio version {} (commit-id={})",
            lock.components.minio.version, lock.components.minio.commit
        ),
        format!(
            "Runtime: go{} {}",
            lock.toolchain.go, lock.components.minio.expected_arch
        ),
    ] {
        if !output.lines().any(|line| line.trim() == expected) {
            return Err(LabError(format!(
                "MinIO output did not prove exact `{expected}`"
            )));
        }
    }
    Ok(())
}

pub fn validate_runner_output(output: &str, lock: &VersionsLock) -> LabResult<()> {
    reject_runtime_state_errors(output)?;
    for expected in [
        format!(
            "uid={} gid={}",
            lock.toolchain.runner_uid, lock.toolchain.runner_gid
        ),
        format!("rustc {} ", lock.toolchain.rust),
        format!("cargo {} ", lock.toolchain.rust),
        format!("v{}", lock.toolchain.node),
        lock.toolchain.npm.clone(),
        format!("tauri-cli {}", lock.toolchain.tauri_cli),
        format!("webkit2gtk-driver {}", lock.toolchain.webkit2gtk_driver),
        format!("libwebkit2gtk-4.1-0:amd64 {}", lock.toolchain.libwebkit2gtk),
        format!("xvfb {}", lock.toolchain.xvfb),
        "HOST_SECRET_PATHS_ABSENT".into(),
    ] {
        if !output.contains(&expected) {
            return Err(LabError(format!(
                "runner output did not prove exact `{expected}`"
            )));
        }
    }
    Ok(())
}

fn exact_trimmed(actual: &str, expected: &str, label: &str) -> LabResult<()> {
    if actual.trim() != expected {
        return Err(LabError(format!(
            "{label} mismatch: expected `{expected}`, got `{}`",
            actual.trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture_lock(repository: &Path) -> VersionsLock {
        VersionsLock::load(
            &repository.join("infra/versions.lock"),
            &repository.join("rust-toolchain.toml"),
        )
        .unwrap()
    }

    fn real_repository() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap()
    }

    #[test]
    fn duplicate_run_id_is_rejected_atomically() {
        let repository = real_repository();
        let lock = fixture_lock(&repository);
        let temporary = tempdir().unwrap();
        fs::create_dir_all(temporary.path().join(".afk/runs")).unwrap();
        let one = RunPlan::create(temporary.path(), Some("duplicate-01"), &lock).unwrap();
        let two = one.clone();
        one.prepare().unwrap();
        assert!(two.prepare().is_err());
    }

    #[test]
    fn run_images_are_scoped_by_repository_and_run() {
        let repository = real_repository();
        let lock = fixture_lock(&repository);
        let one = RunPlan::create(&repository, Some("worker-one"), &lock).unwrap();
        let two = RunPlan::create(&repository, Some("worker-two"), &lock).unwrap();
        assert_ne!(one.images.pocketbase, two.images.pocketbase);
        assert!(one.images.pocketbase.contains("worker-one"));
    }

    #[test]
    fn compose_environment_contains_no_file_or_repo_mount_variable() {
        let repository = real_repository();
        let lock = fixture_lock(&repository);
        let plan = RunPlan::create(&repository, Some("environment-01"), &lock).unwrap();
        let environment = plan.compose_environment(&lock);
        assert!(!environment.contains_key("AFK_REPO_ROOT"));
        assert!(!environment.keys().any(|key| key.contains("ENV_FILE")));
        assert!(!environment.keys().any(|key| key.contains("FIXTURE")));
    }

    #[test]
    fn failure_stage_parser_is_strict() {
        assert_eq!(
            FailureStage::parse("after-start").unwrap(),
            FailureStage::AfterStart
        );
        assert!(FailureStage::parse("anything").is_err());
        assert!(!FailureStage::AfterStart.compatible_with_static());
    }

    #[test]
    fn artifact_enumeration_propagates_directory_errors() {
        let missing = tempdir().unwrap().path().join("missing");
        assert!(enumerate_artifacts(&missing).is_err());
    }

    #[test]
    fn artifact_enumeration_records_stable_length_and_sha256() {
        let temporary = tempdir().unwrap();
        fs::write(temporary.path().join("one.log"), b"abc").unwrap();
        let artifacts = enumerate_artifacts(temporary.path()).unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts["one.log"].bytes, 3);
        assert_eq!(
            artifacts["one.log"].sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn artifact_enumeration_rejects_directories_and_manifest_drift() {
        let temporary = tempdir().unwrap();
        fs::write(temporary.path().join("one.log"), b"abc").unwrap();
        fs::create_dir(temporary.path().join("unexpected-directory")).unwrap();
        assert!(enumerate_artifacts(temporary.path()).is_err());
        fs::remove_dir(temporary.path().join("unexpected-directory")).unwrap();

        let mut declared = enumerate_artifacts(temporary.path()).unwrap();
        fs::write(temporary.path().join("evidence.json"), b"self").unwrap();
        verify_artifact_manifest(temporary.path(), &declared).unwrap();
        fs::write(temporary.path().join("one.log"), b"changed").unwrap();
        assert!(verify_artifact_manifest(temporary.path(), &declared).is_err());
        declared.insert(
            "missing.log".into(),
            artifact_integrity_for_bytes(b"missing"),
        );
        assert!(verify_artifact_manifest(temporary.path(), &declared).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn artifact_enumeration_rejects_symlinks_and_sockets() {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        let temporary = tempdir().unwrap();
        fs::write(temporary.path().join("regular"), b"ok").unwrap();
        symlink("regular", temporary.path().join("linked")).unwrap();
        assert!(enumerate_artifacts(temporary.path()).is_err());
        fs::remove_file(temporary.path().join("linked")).unwrap();
        let _listener = UnixListener::bind(temporary.path().join("socket")).unwrap();
        assert!(enumerate_artifacts(temporary.path()).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn artifact_enumeration_rejects_windows_reparse_symlink_when_available() {
        use std::os::windows::fs::symlink_file;

        let temporary = tempdir().unwrap();
        fs::write(temporary.path().join("regular"), b"ok").unwrap();
        let link = temporary.path().join("linked");
        if symlink_file(temporary.path().join("regular"), &link).is_ok() {
            assert!(enumerate_artifacts(temporary.path()).is_err());
        }
    }

    #[test]
    fn compose_detection_is_deterministic() {
        assert_eq!(
            choose_compose(true, true).unwrap().display(),
            "docker compose"
        );
        assert_eq!(
            choose_compose(false, true).unwrap().display(),
            "docker-compose"
        );
        assert!(choose_compose(false, false).is_err());
    }

    #[test]
    fn redaction_handles_overlap_without_exposing_secrets() {
        let output = redact("abc123 abc", &["abc", "abc123"]);
        assert_eq!(output, "[REDACTED] [REDACTED]");
    }

    #[test]
    fn runtime_resource_proof_requires_exact_unique_cgroup_values() {
        let exact = "AFK_CGROUP_MEMORY=268435456\nAFK_CGROUP_PIDS=128\n";
        validate_runtime_resource_output(exact, "pocketbase").unwrap();
        for invalid in [
            "AFK_CGROUP_MEMORY=max\nAFK_CGROUP_PIDS=128\n",
            "AFK_CGROUP_MEMORY=268435456\nAFK_CGROUP_PIDS=max\n",
            "AFK_CGROUP_MEMORY=268435456\nAFK_CGROUP_PIDS=128\nAFK_CGROUP_PIDS=128\n",
        ] {
            assert!(validate_runtime_resource_output(invalid, "pocketbase").is_err());
        }
    }

    fn exact_runtime_isolation_markers(service: &str, lock: &VersionsLock) -> String {
        let (memory, pids) = expected_service_resource_bounds(service).unwrap();
        let tmpfs = expected_service_tmpfs_bounds(service).unwrap();
        let mut markers = BTreeSet::from([
            format!(
                "AFK_ID={}:{}",
                lock.toolchain.runner_uid, lock.toolchain.runner_gid
            ),
            format!("AFK_CGROUP_MEMORY={memory}"),
            format!("AFK_CGROUP_PIDS={pids}"),
        ]);
        for (target, (size, mode)) in &tmpfs {
            markers.insert(format!(
                "AFK_TMPFS={target}|owner=0:0|mode={mode:o}|size_bytes={size}|options=rw,size|mode_source=stat"
            ));
        }
        for target in expected_service_writable_targets(service).unwrap() {
            let (owner, mode) = if tmpfs.contains_key(&target) {
                ("0:0".to_owned(), 0o1777)
            } else {
                (
                    format!(
                        "{}:{}",
                        lock.toolchain.runner_uid, lock.toolchain.runner_gid
                    ),
                    0o700,
                )
            };
            markers.insert(format!(
                "AFK_WRITABLE={target}|owner={owner}|mode={mode:o}|canary=create-read-delete"
            ));
        }
        markers.into_iter().collect::<Vec<_>>().join("\n") + "\n"
    }

    #[test]
    fn runtime_isolation_proof_is_exact_exhaustive_and_error_sensitive() {
        let lock = fixture_lock(&real_repository());
        for service in ["pocketbase", "minio", "test-runner"] {
            let exact = exact_runtime_isolation_markers(service, &lock);
            let evidence = validate_runtime_isolation_output(&exact, service, &lock).unwrap();
            assert_eq!(
                evidence.writable_targets.len(),
                expected_service_writable_targets(service).unwrap().len()
            );
            let missing = exact.lines().skip(1).collect::<Vec<_>>().join("\n");
            assert!(validate_runtime_isolation_output(&missing, service, &lock).is_err());
            let duplicated = format!("{exact}AFK_CGROUP_PIDS={}\n", evidence.pids);
            assert!(validate_runtime_isolation_output(&duplicated, service, &lock).is_err());
            let state_error = format!("{exact}configuration error: state directory unavailable\n");
            assert!(validate_runtime_isolation_output(&state_error, service, &lock).is_err());
        }
    }

    #[test]
    fn all_failure_manifests_start_with_every_lane() {
        let lanes = initial_lanes();
        assert_eq!(lanes.len(), 4);
        for name in [
            "static-policy",
            "foundation-stack",
            "test-runner",
            "orchestration",
        ] {
            assert!(lanes.contains_key(name));
        }
    }
}
