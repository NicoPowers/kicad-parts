//! MinIO, built from its pinned source, as a disposable loopback S3 service.

use crate::fetch::{Cache, create_dir, publish, sha256_file, unique_suffix};
use crate::pins::NativePins;
use crate::pocketbase::make_executable;
use crate::process::{ServiceProcess, free_loopback_port, run_hidden};
use crate::s3::S3Client;
use crate::{ServiceResult, error, random_token};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Returns a MinIO executable built from the pinned source with the pinned
/// Go toolchain. The first call builds it (several minutes, mostly module
/// downloads verified against the source's go.sum); later calls reuse the
/// cached build while its hash still matches. Where the lock pins the
/// build's SHA-256 (Windows), a build that differs is rejected.
pub fn minio_binary(repository: &Path) -> ServiceResult<PathBuf> {
    static BINARY: Mutex<Option<PathBuf>> = Mutex::new(None);
    let mut cached = BINARY.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(path) = cached.as_ref() {
        return Ok(path.clone());
    }
    let pins = NativePins::load(repository)?;
    let cache = Cache::for_repository(repository);
    let minio = &pins.minio;
    let build_dir = cache.root().join("minio").join(format!(
        "{}-{}-go{}-{}",
        minio.archive.version,
        &minio.archive.sha256[..16],
        pins.go.archive.version,
        pins.platform.goos()
    ));
    let executable = build_dir.join(pins.platform.executable("minio"));
    let stamp = build_dir.join("minio.sha256");
    if executable.is_file()
        && fs::read_to_string(&stamp).ok().as_deref() == Some(sha256_file(&executable)?.as_str())
        && minio
            .build_sha256
            .as_ref()
            .is_none_or(|pinned| fs::read_to_string(&stamp).ok().as_deref() == Some(pinned))
    {
        *cached = Some(executable.clone());
        return Ok(executable);
    }

    let go_root = cache
        .unpacked(&format!("go-{}", pins.go.archive.version), &pins.go.archive)?
        .join("go");
    let source = cache.unpacked(
        &format!("minio-source-{}", minio.short_commit),
        &minio.archive,
    )?;
    create_dir(&build_dir)?;
    let staging = build_dir.join(format!(
        "{}.{}.partial",
        pins.platform.executable("minio"),
        unique_suffix()
    ));
    eprintln!(
        "native-services: building MinIO {} from source with Go {} (first run only)",
        minio.archive.version, pins.go.archive.version
    );
    // The same stamps as infra/docker/minio/Dockerfile.
    let ldflags = format!(
        "-s -w -X github.com/minio/minio/cmd.Version={build} \
         -X github.com/minio/minio/cmd.CopyrightYear={year} \
         -X github.com/minio/minio/cmd.ReleaseTag={release} \
         -X github.com/minio/minio/cmd.CommitID={commit} \
         -X github.com/minio/minio/cmd.ShortCommitID={short} \
         -X github.com/minio/minio/cmd.GOPATH=/go \
         -X github.com/minio/minio/cmd.GOROOT=/usr/local/go",
        build = minio.build_version,
        year = minio.copyright_year,
        release = minio.archive.version,
        commit = minio.commit,
        short = minio.short_commit,
    );
    let go_path = cache.root().join("go-path");
    let mut command = Command::new(go_root.join("bin").join(pins.platform.executable("go")));
    command
        .args([
            "build",
            "-tags",
            "kqueue",
            "-trimpath",
            "-ldflags",
            &ldflags,
            "-o",
        ])
        .arg(&staging)
        .arg(".")
        .current_dir(&source)
        // Only the variables below reach the build, so a developer's own Go
        // settings cannot change what gets built.
        .env_clear()
        .env("GOROOT", &go_root)
        .env("GOPATH", &go_path)
        .env("GOMODCACHE", go_path.join("pkg").join("mod"))
        .env("GOCACHE", cache.root().join("go-build"))
        .env("GOENV", "off")
        .env("GOTOOLCHAIN", "local")
        .env("GOFLAGS", "-mod=readonly")
        .env("GOPROXY", "https://proxy.golang.org")
        .env("GOSUMDB", "sum.golang.org")
        .env("CGO_ENABLED", "0")
        .env("GOOS", pins.platform.goos())
        .env("GOARCH", "amd64")
        .env("GOAMD64", "v1");
    for name in [
        "SystemRoot",
        "SystemDrive",
        "windir",
        "TEMP",
        "TMP",
        "TMPDIR",
        "HOME",
        "PATH",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "NO_PROXY",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let started = Instant::now();
    let output = run_hidden("go build (MinIO)", &mut command)?;
    if !output.status.success() {
        let _ = fs::remove_file(&staging);
        return Err(error(format!(
            "building MinIO failed:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    make_executable(&staging)?;
    let hash = sha256_file(&staging)?;
    if let Some(pinned) = &minio.build_sha256 {
        if &hash != pinned {
            let _ = fs::remove_file(&staging);
            return Err(error(format!(
                "the MinIO build has SHA-256 {hash}, but infra/versions.lock pins {pinned}; \
                 the toolchain, source or build flags differ from the pinned build"
            )));
        }
    }
    publish(&staging, &executable)?;
    fs::write(&stamp, &hash)
        .map_err(|io| error(format!("cannot write `{}`: {io}", stamp.display())))?;
    eprintln!(
        "native-services: built MinIO in {:.0?} (SHA-256 {hash})",
        started.elapsed()
    );
    *cached = Some(executable.clone());
    Ok(executable)
}

/// A running single-drive MinIO server on random 127.0.0.1 ports, with
/// root credentials generated for this run.
pub struct MinioServer {
    process: ServiceProcess,
    endpoint: String,
    access_key: String,
    secret_key: String,
}

impl MinioServer {
    /// Starts MinIO over `data_dir` (which it formats when empty) and waits
    /// until it is ready. `work_dir` holds its log and certificate
    /// directory, so nothing is written to the user's home.
    pub fn start(binary: &Path, data_dir: &Path, work_dir: &Path) -> ServiceResult<Self> {
        create_dir(data_dir)?;
        let certs = work_dir.join("minio-certs");
        create_dir(&certs)?;
        let access_key = random_token(20);
        let secret_key = random_token(40);
        let mut last_failure = String::new();
        for _ in 0..3 {
            let port = free_loopback_port()?;
            let console_port = free_loopback_port()?;
            let mut command = Command::new(binary);
            command
                .arg("server")
                .arg(data_dir)
                .arg(format!("--address=127.0.0.1:{port}"))
                .arg(format!("--console-address=127.0.0.1:{console_port}"))
                .arg("--certs-dir")
                .arg(&certs);
            for (name, _) in std::env::vars_os() {
                if name
                    .to_string_lossy()
                    .to_ascii_uppercase()
                    .starts_with("MINIO_")
                {
                    command.env_remove(name);
                }
            }
            command
                .env("MINIO_ROOT_USER", &access_key)
                .env("MINIO_ROOT_PASSWORD", &secret_key)
                .env("MINIO_BROWSER", "off")
                .env("MINIO_UPDATE", "off")
                .env("MINIO_CI_CD", "on");
            let mut process = ServiceProcess::spawn("minio", command, &work_dir.join("minio.log"))?;
            let endpoint = format!("http://127.0.0.1:{port}");
            match wait_until_live(&mut process, &endpoint, Duration::from_secs(60)) {
                Ok(()) => {
                    return Ok(Self {
                        process,
                        endpoint,
                        access_key,
                        secret_key,
                    });
                }
                Err(failure) => {
                    last_failure = failure.0;
                    let log = process.log_tail();
                    if !log.contains("bind") && !log.contains("already in use") {
                        break;
                    }
                }
            }
        }
        Err(error(format!("MinIO did not start: {last_failure}")))
    }

    /// The S3 endpoint, `http://127.0.0.1:<port>`.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn access_key(&self) -> &str {
        &self.access_key
    }

    pub fn secret_key(&self) -> &str {
        &self.secret_key
    }

    /// An S3 client signed with this server's root credentials.
    pub fn s3(&self) -> S3Client {
        S3Client::new(&self.endpoint, &self.access_key, &self.secret_key)
    }

    pub fn log_tail(&self) -> String {
        self.process.log_tail()
    }

    pub fn stop(mut self) {
        self.process.stop();
    }
}

fn wait_until_live(
    process: &mut ServiceProcess,
    endpoint: &str,
    timeout: Duration,
) -> ServiceResult<()> {
    // "live" answers before the drive is initialised; "ready" waits for it.
    let ready = format!("{endpoint}/minio/health/ready");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(2))
        .build();
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = process.exited() {
            return Err(error(format!(
                "minio exited with {status}:\n{}",
                process.log_tail()
            )));
        }
        if matches!(agent.get(&ready).call(), Ok(response) if response.status() == 200) {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(error(format!(
                "minio was not ready after {timeout:?}:\n{}",
                process.log_tail()
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
