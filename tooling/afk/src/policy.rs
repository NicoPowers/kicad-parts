use crate::{LabError, LabResult, compose_path};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct PolicyExpectation {
    pub project: String,
    pub fixture_dir: PathBuf,
    pub artifact_dir: PathBuf,
    pub user: String,
    pub images: BTreeMap<String, String>,
    pub build_contexts: BTreeMap<String, PathBuf>,
    pub build_args: BTreeMap<String, BTreeMap<String, String>>,
    pub minio_user: String,
    pub minio_password: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BindMountEvidence {
    pub service: String,
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ComposePolicyReport {
    pub passed: bool,
    pub services: Vec<String>,
    pub bind_mounts: Vec<BindMountEvidence>,
    pub named_volumes: Vec<String>,
    pub internal_network: String,
    pub host_ports: usize,
    pub forbidden_host_paths_mounted: bool,
    pub service_resources: BTreeMap<String, ServiceResourceEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ServiceResourceEvidence {
    pub memory_bytes: u64,
    pub pids: u64,
    pub tmpfs: BTreeMap<String, TmpfsEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TmpfsEvidence {
    pub size_bytes: u64,
    pub mode: u64,
}

pub fn validate_compose_policy(
    rendered: &str,
    expected: &PolicyExpectation,
) -> LabResult<ComposePolicyReport> {
    let root: Value = serde_json::from_str(rendered)?;
    let root = object(&root, "root")?;
    exact_keys(root, &["name", "networks", "services", "volumes"], "root")?;
    string(root, "name", "root")?
        .eq(&expected.project)
        .then_some(())
        .ok_or_else(|| LabError("normalized Compose project name is not run-scoped".into()))?;
    reject_forbidden_keys(&Value::Object(root.clone()), "root")?;

    let services = object(required(root, "services", "root")?, "services")?;
    exact_keys(
        services,
        &["kicad-cli", "minio", "pocketbase", "test-runner"],
        "services",
    )?;
    let mut binds = Vec::new();
    let mut service_resources = BTreeMap::new();
    for name in ["pocketbase", "minio", "test-runner", "kicad-cli"] {
        let resources = validate_service(
            name,
            required(services, name, "services")?,
            expected,
            &mut binds,
        )?;
        service_resources.insert(name.into(), resources);
    }

    let volumes = object(required(root, "volumes", "root")?, "volumes")?;
    exact_keys(volumes, &["minio-data", "pocketbase-data"], "volumes")?;
    for name in ["minio-data", "pocketbase-data"] {
        let volume = object(
            required(volumes, name, "volumes")?,
            &format!("volumes.{name}"),
        )?;
        exact_keys(volume, &["name"], &format!("volumes.{name}"))?;
        let actual = string(volume, "name", &format!("volumes.{name}"))?;
        let wanted = format!("{}_{}", expected.project, name);
        if actual != wanted {
            return Err(LabError(format!(
                "volume `{name}` has non-scoped name `{actual}`"
            )));
        }
    }

    let networks = object(required(root, "networks", "root")?, "networks")?;
    exact_keys(networks, &["afk"], "networks")?;
    let network = object(required(networks, "afk", "networks")?, "networks.afk")?;
    exact_keys(network, &["internal", "ipam", "name"], "networks.afk")?;
    if required(network, "internal", "networks.afk")?.as_bool() != Some(true) {
        return Err(LabError("AFK network must be internal".into()));
    }
    if !object(
        required(network, "ipam", "networks.afk")?,
        "networks.afk.ipam",
    )?
    .is_empty()
    {
        return Err(LabError(
            "AFK network IPAM must use isolated defaults".into(),
        ));
    }
    let network_name = string(network, "name", "networks.afk")?;
    let wanted_network = format!("{}_afk", expected.project);
    if network_name != wanted_network {
        return Err(LabError(
            "AFK network name is fixed or not run-scoped".into(),
        ));
    }

    validate_bind_paths(expected, &binds)?;
    Ok(ComposePolicyReport {
        passed: true,
        services: services.keys().cloned().collect(),
        bind_mounts: binds,
        named_volumes: volumes.keys().cloned().collect(),
        internal_network: network_name.to_owned(),
        host_ports: 0,
        forbidden_host_paths_mounted: false,
        service_resources,
    })
}

fn validate_service(
    name: &str,
    value: &Value,
    expected: &PolicyExpectation,
    binds: &mut Vec<BindMountEvidence>,
) -> LabResult<ServiceResourceEvidence> {
    let service = object(value, &format!("services.{name}"))?;
    let keys: &[&str] = match name {
        "pocketbase" | "minio" => &[
            "build",
            "command",
            "entrypoint",
            "environment",
            "healthcheck",
            "image",
            "mem_limit",
            "networks",
            "pids_limit",
            "read_only",
            "user",
            "volumes",
        ],
        "test-runner" => &[
            "build",
            "command",
            "entrypoint",
            "environment",
            "image",
            "mem_limit",
            "networks",
            "pids_limit",
            "read_only",
            "user",
            "volumes",
        ],
        "kicad-cli" => &[
            "command",
            "entrypoint",
            "environment",
            "image",
            "mem_limit",
            "networks",
            "pids_limit",
            "read_only",
            "user",
            "volumes",
        ],
        _ => return Err(LabError(format!("unexpected service `{name}`"))),
    };
    exact_keys(service, keys, &format!("services.{name}"))?;
    if required(service, "read_only", name)?.as_bool() != Some(true) {
        return Err(LabError(format!("service `{name}` root must be read-only")));
    }
    if string(service, "user", name)? != expected.user {
        return Err(LabError(format!(
            "service `{name}` must run as {}",
            expected.user
        )));
    }
    if string(service, "image", name)?
        != expected
            .images
            .get(name)
            .ok_or_else(|| LabError(format!("missing expected image for `{name}`")))?
    {
        return Err(LabError(format!(
            "service `{name}` image does not match the run plan"
        )));
    }
    validate_network_attachment(name, required(service, "networks", name)?)?;
    validate_environment(name, required(service, "environment", name)?, expected)?;
    let resources = validate_resource_bounds(name, service)?;
    let volumes = required(service, "volumes", name)?;
    validate_command(name, required(service, "command", name)?)?;
    validate_entrypoint(name, required(service, "entrypoint", name)?)?;
    if matches!(name, "pocketbase" | "minio") {
        validate_healthcheck(name, required(service, "healthcheck", name)?)?;
    }
    if name != "kicad-cli" {
        validate_build(name, required(service, "build", name)?, expected)?;
    }
    match name {
        "pocketbase" => validate_named_volume(name, volumes, "pocketbase-data", "/afk/pb_data")?,
        "minio" => validate_named_volume(name, volumes, "minio-data", "/afk/minio-data")?,
        "kicad-cli" => validate_kicad_binds(volumes, expected, binds)?,
        "test-runner" => validate_only_tmpfs(name, volumes)?,
        _ => unreachable!(),
    }
    Ok(resources)
}

fn validate_entrypoint(name: &str, value: &Value) -> LabResult<()> {
    let valid = if name == "kicad-cli" {
        value.as_array().is_some_and(Vec::is_empty)
    } else {
        value.is_null()
    };
    if !valid {
        return Err(LabError(format!(
            "service `{name}` entrypoint override is unexpected"
        )));
    }
    Ok(())
}

fn validate_healthcheck(name: &str, value: &Value) -> LabResult<()> {
    let health = object(value, &format!("services.{name}.healthcheck"))?;
    exact_keys(
        health,
        &["interval", "retries", "start_period", "test", "timeout"],
        &format!("services.{name}.healthcheck"),
    )?;
    for key in ["interval", "start_period", "timeout"] {
        if string(health, key, name)? != "2s" {
            return Err(LabError(format!(
                "service `{name}` healthcheck `{key}` is not bounded"
            )));
        }
    }
    if required(health, "retries", name)?.as_u64() != Some(30) {
        return Err(LabError(format!(
            "service `{name}` healthcheck retries changed"
        )));
    }
    let test = required(health, "test", name)?
        .as_array()
        .ok_or_else(|| {
            LabError(format!(
                "service `{name}` healthcheck test must be exec-form"
            ))
        })?
        .iter()
        .map(|value| value.as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    let url = if name == "pocketbase" {
        "http://127.0.0.1:8090/api/health"
    } else {
        "http://127.0.0.1:9000/minio/health/live"
    };
    if test != ["CMD", "wget", "-q", "-O", "/dev/null", url] {
        return Err(LabError(format!(
            "service `{name}` healthcheck command changed"
        )));
    }
    Ok(())
}

fn validate_build(name: &str, value: &Value, expected: &PolicyExpectation) -> LabResult<()> {
    let build = object(value, &format!("services.{name}.build"))?;
    exact_keys(
        build,
        &["args", "context", "dockerfile"],
        &format!("services.{name}.build"),
    )?;
    if string(build, "dockerfile", name)? != "Dockerfile" {
        return Err(LabError(format!(
            "service `{name}` has unexpected Dockerfile"
        )));
    }
    let actual_context = canonical_checked(Path::new(string(build, "context", name)?))?;
    let wanted = canonical_checked(
        expected
            .build_contexts
            .get(name)
            .ok_or_else(|| LabError(format!("missing expected build context for `{name}`")))?,
    )?;
    if normalize(&actual_context) != normalize(&wanted) {
        return Err(LabError(format!(
            "service `{name}` build context escaped its Dockerfile directory"
        )));
    }
    let args = object(
        required(build, "args", name)?,
        &format!("services.{name}.build.args"),
    )?;
    let wanted_args = expected
        .build_args
        .get(name)
        .ok_or_else(|| LabError(format!("missing expected build arguments for `{name}`")))?;
    if args.len() != wanted_args.len() {
        return Err(LabError(format!(
            "service `{name}` build arguments are not exhaustive"
        )));
    }
    for (key, wanted) in wanted_args {
        if string(args, key, name)? != wanted {
            return Err(LabError(format!(
                "service `{name}` build arg `{key}` mismatches the lock"
            )));
        }
    }
    Ok(())
}

fn validate_environment(name: &str, value: &Value, expected: &PolicyExpectation) -> LabResult<()> {
    let environment = object(value, &format!("services.{name}.environment"))?;
    let mut wanted = BTreeMap::from([
        ("HOME", "/afk/home"),
        ("XDG_CACHE_HOME", "/afk/xdg/cache"),
        ("XDG_CONFIG_HOME", "/afk/xdg/config"),
        ("XDG_DATA_HOME", "/afk/xdg/data"),
        ("XDG_STATE_HOME", "/afk/xdg/state"),
    ]);
    if matches!(name, "test-runner" | "kicad-cli") {
        wanted.insert("KICAD_CONFIG_HOME", "/afk/kicad/config");
    }
    if name == "minio" {
        wanted.insert("MINIO_BROWSER", "off");
    }
    let expected_len = wanted.len() + usize::from(name == "minio") * 2;
    if environment.len() != expected_len {
        return Err(LabError(format!(
            "service `{name}` environment contains unexpected keys"
        )));
    }
    for (key, wanted_value) in wanted {
        if string(environment, key, name)? != wanted_value {
            return Err(LabError(format!(
                "service `{name}` environment `{key}` is unexpected"
            )));
        }
    }
    if name == "minio"
        && (string(environment, "MINIO_ROOT_USER", name)? != expected.minio_user
            || string(environment, "MINIO_ROOT_PASSWORD", name)? != expected.minio_password)
    {
        return Err(LabError(
            "MinIO credentials are not the in-memory run credentials".into(),
        ));
    }
    Ok(())
}

pub fn expected_service_resource_bounds(name: &str) -> LabResult<(u64, u64)> {
    match name {
        "pocketbase" => Ok((256 * 1024 * 1024, 128)),
        "minio" => Ok((1024 * 1024 * 1024, 256)),
        "test-runner" | "kicad-cli" => Ok((2 * 1024 * 1024 * 1024, 512)),
        _ => Err(LabError(format!("unexpected service `{name}`"))),
    }
}

fn expected_tmpfs(name: &str) -> LabResult<BTreeMap<&'static str, (u64, u64)>> {
    let mut wanted = BTreeMap::from([
        ("/tmp", (128 * 1024 * 1024, 0o1777)),
        ("/afk/home", (64 * 1024 * 1024, 0o1777)),
        ("/afk/xdg", (64 * 1024 * 1024, 0o1777)),
    ]);
    match name {
        "test-runner" | "kicad-cli" => {
            wanted.insert("/afk/kicad", (64 * 1024 * 1024, 0o1777));
        }
        "pocketbase" | "minio" => {}
        _ => return Err(LabError(format!("unexpected service `{name}`"))),
    }
    Ok(wanted)
}

pub fn expected_service_tmpfs_bounds(name: &str) -> LabResult<BTreeMap<String, (u64, u64)>> {
    Ok(expected_tmpfs(name)?
        .into_iter()
        .map(|(target, bounds)| (target.to_owned(), bounds))
        .collect())
}

fn validate_resource_bounds(
    name: &str,
    service: &Map<String, Value>,
) -> LabResult<ServiceResourceEvidence> {
    let (wanted_memory, wanted_pids) = expected_service_resource_bounds(name)?;
    let memory = normalized_u64(required(service, "mem_limit", name)?, name)?;
    let pids = required(service, "pids_limit", name)?
        .as_u64()
        .ok_or_else(|| LabError(format!("service `{name}` PID limit must be an integer")))?;
    if memory != wanted_memory || pids != wanted_pids {
        return Err(LabError(format!(
            "service `{name}` resource limits changed: memory={memory}, pids={pids}"
        )));
    }
    let tmpfs = validate_tmpfs(name, required(service, "volumes", name)?)?;
    Ok(ServiceResourceEvidence {
        memory_bytes: memory,
        pids,
        tmpfs,
    })
}

fn validate_tmpfs(name: &str, value: &Value) -> LabResult<BTreeMap<String, TmpfsEvidence>> {
    let values = value
        .as_array()
        .ok_or_else(|| LabError(format!("service `{name}` volumes must be an array")))?;
    let wanted = expected_tmpfs(name)?;
    let mut actual = BTreeMap::new();
    for (index, value) in values.iter().enumerate() {
        let mount = object(value, &format!("services.{name}.volumes[{index}]"))?;
        if string(mount, "type", name)? != "tmpfs" {
            continue;
        }
        exact_keys(mount, &["target", "tmpfs", "type"], name)?;
        let target = string(mount, "target", name)?;
        let options = object(required(mount, "tmpfs", name)?, name)?;
        exact_keys(options, &["mode", "size"], name)?;
        let size = normalized_u64(required(options, "size", name)?, name)?;
        let mode = required(options, "mode", name)?
            .as_u64()
            .ok_or_else(|| LabError(format!("service `{name}` tmpfs mode must be an integer")))?;
        if actual
            .insert(
                target.to_owned(),
                TmpfsEvidence {
                    size_bytes: size,
                    mode,
                },
            )
            .is_some()
        {
            return Err(LabError(format!(
                "service `{name}` has duplicate tmpfs target `{target}`"
            )));
        }
    }
    let expected = wanted
        .into_iter()
        .map(|(target, (size_bytes, mode))| (target.to_owned(), TmpfsEvidence { size_bytes, mode }))
        .collect::<BTreeMap<_, _>>();
    if actual != expected {
        return Err(LabError(format!(
            "service `{name}` tmpfs bounds differ: actual={actual:?}, expected={expected:?}"
        )));
    }
    Ok(actual)
}

fn validate_command(name: &str, value: &Value) -> LabResult<()> {
    let actual = value
        .as_array()
        .ok_or_else(|| LabError(format!("service `{name}` command must be exec-form")))?
        .iter()
        .map(|value| value.as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    let wanted = match name {
        "pocketbase" => vec!["serve", "--http=0.0.0.0:8090", "--dir=/afk/pb_data"],
        "minio" => vec![
            "server",
            "/afk/minio-data",
            "--address",
            ":9000",
            "--console-address",
            ":9001",
        ],
        "test-runner" => vec!["sleep", "infinity"],
        "kicad-cli" => vec!["kicad-cli", "version", "--format", "about"],
        _ => unreachable!(),
    };
    if actual != wanted {
        return Err(LabError(format!("service `{name}` command is unexpected")));
    }
    Ok(())
}

fn validate_network_attachment(name: &str, value: &Value) -> LabResult<()> {
    let networks = object(value, &format!("services.{name}.networks"))?;
    exact_keys(networks, &["afk"], &format!("services.{name}.networks"))?;
    if !required(networks, "afk", name)?.is_null() {
        return Err(LabError(format!(
            "service `{name}` network attachment has overrides"
        )));
    }
    Ok(())
}

fn validate_named_volume(
    service: &str,
    value: &Value,
    source: &str,
    target: &str,
) -> LabResult<()> {
    let volumes = value
        .as_array()
        .ok_or_else(|| LabError(format!("service `{service}` volumes must be an array")))?;
    let tmpfs_count = expected_tmpfs(service)?.len();
    if volumes.len() != tmpfs_count + 1 {
        return Err(LabError(format!(
            "service `{service}` must have exactly one named volume and {tmpfs_count} tmpfs mounts"
        )));
    }
    let persistent = volumes
        .iter()
        .filter(|value| {
            value
                .as_object()
                .and_then(|mount| mount.get("type"))
                .and_then(Value::as_str)
                == Some("volume")
        })
        .collect::<Vec<_>>();
    if persistent.len() != 1 {
        return Err(LabError(format!(
            "service `{service}` must have exactly one named volume"
        )));
    }
    let volume = object(persistent[0], &format!("services.{service}.named-volume"))?;
    exact_keys(volume, &["source", "target", "type", "volume"], service)?;
    if string(volume, "type", service)? != "volume"
        || string(volume, "source", service)? != source
        || string(volume, "target", service)? != target
        || !object(required(volume, "volume", service)?, service)?.is_empty()
    {
        return Err(LabError(format!(
            "service `{service}` named volume is unexpected"
        )));
    }
    Ok(())
}

fn validate_only_tmpfs(service: &str, value: &Value) -> LabResult<()> {
    let volumes = value
        .as_array()
        .ok_or_else(|| LabError(format!("service `{service}` volumes must be an array")))?;
    if volumes.len() != expected_tmpfs(service)?.len()
        || volumes.iter().any(|value| {
            value
                .as_object()
                .and_then(|mount| mount.get("type"))
                .and_then(Value::as_str)
                != Some("tmpfs")
        })
    {
        return Err(LabError(format!(
            "service `{service}` must contain only its exact tmpfs mounts"
        )));
    }
    Ok(())
}

fn validate_kicad_binds(
    value: &Value,
    expected: &PolicyExpectation,
    binds: &mut Vec<BindMountEvidence>,
) -> LabResult<()> {
    let volumes = value
        .as_array()
        .ok_or_else(|| LabError("kicad-cli volumes must be an array".into()))?;
    let tmpfs_count = expected_tmpfs("kicad-cli")?.len();
    if volumes.len() != tmpfs_count + 2 {
        return Err(LabError(
            "kicad-cli must have exactly fixture/artifact binds and bounded tmpfs mounts".into(),
        ));
    }
    let bind_mounts = volumes
        .iter()
        .filter(|value| {
            value
                .as_object()
                .and_then(|mount| mount.get("type"))
                .and_then(Value::as_str)
                == Some("bind")
        })
        .collect::<Vec<_>>();
    if bind_mounts.len() != 2 {
        return Err(LabError(
            "kicad-cli must have exactly two bind mounts".into(),
        ));
    }
    let wanted = [
        (&expected.fixture_dir, "/fixture", true),
        (&expected.artifact_dir, "/artifacts", false),
    ];
    for (wanted_source, wanted_target, wanted_ro) in wanted {
        let mount = bind_mounts
            .iter()
            .find_map(|value| {
                value.as_object().filter(|mount| {
                    mount.get("target").and_then(Value::as_str) == Some(wanted_target)
                })
            })
            .ok_or_else(|| LabError(format!("kicad-cli bind `{wanted_target}` is missing")))?;
        let keys = if wanted_ro {
            &["read_only", "source", "target", "type"][..]
        } else {
            &["source", "target", "type"][..]
        };
        exact_keys(mount, keys, "kicad-cli volume")?;
        if string(mount, "type", "kicad-cli volume")? != "bind"
            || string(mount, "target", "kicad-cli volume")? != wanted_target
            || mount
                .get("read_only")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                != wanted_ro
        {
            return Err(LabError(
                "kicad-cli bind target or mode is unexpected".into(),
            ));
        }
        let actual = canonical_checked(Path::new(string(mount, "source", "kicad-cli volume")?))?;
        let wanted = canonical_checked(wanted_source)?;
        if normalize(&actual) != normalize(&wanted) {
            return Err(LabError(
                "kicad-cli bind source is not the exact allowlisted path".into(),
            ));
        }
        binds.push(BindMountEvidence {
            service: "kicad-cli".into(),
            source: compose_path(&actual),
            target: wanted_target.into(),
            read_only: wanted_ro,
        });
    }
    Ok(())
}

fn validate_bind_paths(expected: &PolicyExpectation, binds: &[BindMountEvidence]) -> LabResult<()> {
    let fixture = canonical_checked(&expected.fixture_dir)?;
    let artifacts = canonical_checked(&expected.artifact_dir)?;
    if path_overlaps(&fixture, &artifacts) {
        return Err(LabError("fixture and artifact bind sources overlap".into()));
    }
    if expected.fixture_dir.join("secrets.env").exists() {
        return Err(LabError(
            "safe fixture directory unexpectedly contains secrets.env".into(),
        ));
    }
    if binds.len() != 2 {
        return Err(LabError("normalized bind allowlist is incomplete".into()));
    }
    Ok(())
}

fn reject_forbidden_keys(value: &Value, path: &str) -> LabResult<()> {
    const FORBIDDEN: &[&str] = &[
        "cap_add",
        "configs",
        "devices",
        "env_file",
        "external_links",
        "extra_hosts",
        "ipc",
        "network_mode",
        "pid",
        "ports",
        "privileged",
        "security_opt",
        "secrets",
    ];
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if FORBIDDEN.contains(&key.as_str()) {
                    return Err(LabError(format!("forbidden Compose key `{path}.{key}`")));
                }
                if key == "source"
                    && child
                        .as_str()
                        .is_some_and(|source| source.contains("docker.sock"))
                {
                    return Err(LabError("Docker socket mount is forbidden".into()));
                }
                if key == "external" {
                    return Err(LabError(format!(
                        "external resource key `{path}.external` is forbidden"
                    )));
                }
                reject_forbidden_keys(child, &format!("{path}.{key}"))?;
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                reject_forbidden_keys(child, &format!("{path}[{index}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn canonical_checked(path: &Path) -> LabResult<PathBuf> {
    let canonical = path
        .canonicalize()
        .map_err(|error| LabError(format!("cannot canonicalize `{}`: {error}", path.display())))?;
    let mut cursor = Some(canonical.as_path());
    while let Some(candidate) = cursor {
        let metadata = fs::symlink_metadata(candidate)?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(LabError(format!(
                "allowlisted path contains a link/reparse point: `{}`",
                candidate.display()
            )));
        }
        cursor = candidate.parent();
    }
    Ok(canonical)
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

fn path_overlaps(one: &Path, two: &Path) -> bool {
    let one = normalize(one);
    let two = normalize(two);
    one == two || one.starts_with(&(two.clone() + "/")) || two.starts_with(&(one + "/"))
}

fn normalize(path: &Path) -> String {
    let value = compose_path(path).trim_end_matches('/').to_owned();
    #[cfg(windows)]
    {
        value.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        value
    }
}

fn object<'a>(value: &'a Value, path: &str) -> LabResult<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| LabError(format!("`{path}` must be an object")))
}

fn required<'a>(map: &'a Map<String, Value>, key: &str, path: &str) -> LabResult<&'a Value> {
    map.get(key)
        .ok_or_else(|| LabError(format!("`{path}.{key}` is required")))
}

fn string<'a>(map: &'a Map<String, Value>, key: &str, path: &str) -> LabResult<&'a str> {
    required(map, key, path)?
        .as_str()
        .ok_or_else(|| LabError(format!("`{path}.{key}` must be a string")))
}

fn normalized_u64(value: &Value, path: &str) -> LabResult<u64> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| LabError(format!("`{path}` normalized byte value must be unsigned"))),
        Value::String(text) => text
            .parse::<u64>()
            .map_err(|_| LabError(format!("`{path}` normalized byte string is invalid"))),
        _ => Err(LabError(format!(
            "`{path}` normalized byte value must be an integer or integer string"
        ))),
    }
}

fn exact_keys(map: &Map<String, Value>, wanted: &[&str], path: &str) -> LabResult<()> {
    let actual = map.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let wanted = wanted.iter().copied().collect::<BTreeSet<_>>();
    if actual != wanted {
        return Err(LabError(format!(
            "`{path}` keys differ: actual={actual:?}, wanted={wanted:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    type PolicyMutation = Box<dyn Fn(&mut Value, &mut PolicyExpectation)>;

    fn setup() -> (TempDir, PolicyExpectation, Value) {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let fixture = root.join("fixture");
        let artifacts = root.join("runs/run-01/artifacts");
        for path in [
            &fixture,
            &artifacts,
            &root.join("infra/docker/pocketbase"),
            &root.join("infra/docker/minio"),
            &root.join("infra/docker/test-runner"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        let images = BTreeMap::from([
            ("pocketbase".into(), "run/pocketbase:1".into()),
            ("minio".into(), "run/minio:1".into()),
            ("test-runner".into(), "run/test-runner:1".into()),
            ("kicad-cli".into(), "kicad@sha256:locked".into()),
        ]);
        let build_contexts = BTreeMap::from([
            ("pocketbase".into(), root.join("infra/docker/pocketbase")),
            ("minio".into(), root.join("infra/docker/minio")),
            ("test-runner".into(), root.join("infra/docker/test-runner")),
        ]);
        let build_args = BTreeMap::from([
            (
                "pocketbase".into(),
                BTreeMap::from([("PIN".into(), "pb".into())]),
            ),
            (
                "minio".into(),
                BTreeMap::from([("PIN".into(), "minio".into())]),
            ),
            (
                "test-runner".into(),
                BTreeMap::from([("PIN".into(), "runner".into())]),
            ),
        ]);
        let expected = PolicyExpectation {
            project: "kp-afk-run-01".into(),
            fixture_dir: fixture,
            artifact_dir: artifacts,
            user: "65532:65532".into(),
            images,
            build_contexts,
            build_args,
            minio_user: "scoped-user".into(),
            minio_password: "scoped-password".into(),
        };
        let base_env = json!({
            "HOME":"/afk/home", "XDG_CACHE_HOME":"/afk/xdg/cache",
            "XDG_CONFIG_HOME":"/afk/xdg/config", "XDG_DATA_HOME":"/afk/xdg/data",
            "XDG_STATE_HOME":"/afk/xdg/state"
        });
        let build = |name: &str| {
            json!({
                "context": compose_path(&expected.build_contexts[name]), "dockerfile":"Dockerfile",
                "args": expected.build_args[name]
            })
        };
        let health = |url: &str| {
            json!({
                "test":["CMD","wget","-q","-O","/dev/null",url], "timeout":"2s",
                "interval":"2s", "retries":30, "start_period":"2s"
            })
        };
        let mut runner_env = base_env.clone();
        runner_env["KICAD_CONFIG_HOME"] = json!("/afk/kicad/config");
        let mut minio_env = base_env.clone();
        minio_env["MINIO_BROWSER"] = json!("off");
        minio_env["MINIO_ROOT_USER"] = json!(expected.minio_user);
        minio_env["MINIO_ROOT_PASSWORD"] = json!(expected.minio_password);
        let model = json!({
            "name": expected.project,
            "networks":{"afk":{"name":"kp-afk-run-01_afk","ipam":{},"internal":true}},
            "volumes":{
                "minio-data":{"name":"kp-afk-run-01_minio-data"},
                "pocketbase-data":{"name":"kp-afk-run-01_pocketbase-data"}
            },
            "services":{
                "pocketbase":{
                    "build":build("pocketbase"), "command":["serve","--http=0.0.0.0:8090","--dir=/afk/pb_data"],
                    "entrypoint":null, "environment":base_env, "healthcheck":health("http://127.0.0.1:8090/api/health"),
                    "image":expected.images["pocketbase"], "mem_limit":"268435456", "networks":{"afk":null},
                    "pids_limit":128, "read_only":true, "user":expected.user,
                    "volumes":[
                        {"type":"volume","source":"pocketbase-data","target":"/afk/pb_data","volume":{}},
                        {"type":"tmpfs","target":"/tmp","tmpfs":{"size":"134217728","mode":1023}},
                        {"type":"tmpfs","target":"/afk/home","tmpfs":{"size":"67108864","mode":1023}},
                        {"type":"tmpfs","target":"/afk/xdg","tmpfs":{"size":"67108864","mode":1023}}
                    ]
                },
                "minio":{
                    "build":build("minio"), "command":["server","/afk/minio-data","--address",":9000","--console-address",":9001"],
                    "entrypoint":null, "environment":minio_env, "healthcheck":health("http://127.0.0.1:9000/minio/health/live"),
                    "image":expected.images["minio"], "mem_limit":"1073741824", "networks":{"afk":null},
                    "pids_limit":256, "read_only":true, "user":expected.user,
                    "volumes":[
                        {"type":"volume","source":"minio-data","target":"/afk/minio-data","volume":{}},
                        {"type":"tmpfs","target":"/tmp","tmpfs":{"size":"134217728","mode":1023}},
                        {"type":"tmpfs","target":"/afk/home","tmpfs":{"size":"67108864","mode":1023}},
                        {"type":"tmpfs","target":"/afk/xdg","tmpfs":{"size":"67108864","mode":1023}}
                    ]
                },
                "test-runner":{
                    "build":build("test-runner"), "command":["sleep","infinity"], "entrypoint":null,
                    "environment":runner_env, "image":expected.images["test-runner"], "networks":{"afk":null},
                    "mem_limit":"2147483648", "pids_limit":512, "read_only":true, "user":expected.user,
                    "volumes":[
                        {"type":"tmpfs","target":"/tmp","tmpfs":{"size":"134217728","mode":1023}},
                        {"type":"tmpfs","target":"/afk/home","tmpfs":{"size":"67108864","mode":1023}},
                        {"type":"tmpfs","target":"/afk/xdg","tmpfs":{"size":"67108864","mode":1023}},
                        {"type":"tmpfs","target":"/afk/kicad","tmpfs":{"size":"67108864","mode":1023}}
                    ]
                },
                "kicad-cli":{
                    "command":["kicad-cli","version","--format","about"], "entrypoint":[], "environment":runner_env,
                    "image":expected.images["kicad-cli"], "mem_limit":"2147483648", "networks":{"afk":null},
                    "pids_limit":512, "read_only":true, "user":expected.user,
                    "volumes":[
                        {"type":"bind","source":compose_path(&expected.fixture_dir),"target":"/fixture","read_only":true},
                        {"type":"bind","source":compose_path(&expected.artifact_dir),"target":"/artifacts"},
                        {"type":"tmpfs","target":"/tmp","tmpfs":{"size":"134217728","mode":1023}},
                        {"type":"tmpfs","target":"/afk/home","tmpfs":{"size":"67108864","mode":1023}},
                        {"type":"tmpfs","target":"/afk/xdg","tmpfs":{"size":"67108864","mode":1023}},
                        {"type":"tmpfs","target":"/afk/kicad","tmpfs":{"size":"67108864","mode":1023}}
                    ]
                }
            }
        });
        (temporary, expected, model)
    }

    #[test]
    fn exact_normalized_model_is_accepted() {
        let (_temporary, expected, model) = setup();
        let report = validate_compose_policy(&model.to_string(), &expected).unwrap();
        assert_eq!(report.bind_mounts.len(), 2);
        assert_eq!(report.host_ports, 0);
    }

    #[test]
    fn equivalent_numeric_normalization_from_compose_variants_is_accepted() {
        let (_temporary, expected, mut model) = setup();
        for service in ["pocketbase", "minio", "test-runner", "kicad-cli"] {
            let memory = model["services"][service]["mem_limit"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            model["services"][service]["mem_limit"] = json!(memory);
            for mount in model["services"][service]["volumes"]
                .as_array_mut()
                .unwrap()
            {
                if mount["type"] == "tmpfs" {
                    let size = mount["tmpfs"]["size"]
                        .as_str()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap();
                    mount["tmpfs"]["size"] = json!(size);
                }
            }
        }
        validate_compose_policy(&model.to_string(), &expected).unwrap();
    }

    #[test]
    fn every_dangerous_compose_escape_key_is_rejected() {
        for key in [
            "ports",
            "network_mode",
            "pid",
            "ipc",
            "devices",
            "privileged",
            "cap_add",
            "env_file",
            "secrets",
            "configs",
            "extra_hosts",
            "security_opt",
            "external_links",
        ] {
            let (_temporary, expected, mut model) = setup();
            model["services"]["test-runner"]
                .as_object_mut()
                .unwrap()
                .insert(key.into(), json!(true));
            assert!(
                validate_compose_policy(&model.to_string(), &expected).is_err(),
                "accepted `{key}`"
            );
        }
    }

    #[test]
    fn resource_and_mount_escape_classes_are_rejected() {
        let mutations: Vec<PolicyMutation> = vec![
            Box::new(|model, _| {
                model
                    .as_object_mut()
                    .unwrap()
                    .insert("x-extension".into(), json!({}));
            }),
            Box::new(|model, _| {
                model["services"]
                    .as_object_mut()
                    .unwrap()
                    .insert("surprise".into(), json!({}));
            }),
            Box::new(|model, _| {
                model["volumes"]["minio-data"]
                    .as_object_mut()
                    .unwrap()
                    .insert("external".into(), json!(true));
            }),
            Box::new(|model, _| {
                model["networks"]["afk"]["internal"] = json!(false);
            }),
            Box::new(|model, _| {
                model["networks"]["afk"]["name"] = json!("fixed-network");
            }),
            Box::new(|model, _| {
                model["volumes"]["minio-data"]["name"] = json!("fixed-volume");
            }),
            Box::new(|model, _| {
                model["services"]["test-runner"]["user"] = json!("0:0");
            }),
            Box::new(|model, _| {
                model["services"]["test-runner"]["environment"]["HOME"] = json!("/root");
            }),
            Box::new(|model, _| {
                model["services"]["test-runner"]["environment"]["EVIL"] = json!("1");
            }),
            Box::new(|model, _| {
                model["services"]["test-runner"]["volumes"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"type":"tmpfs","target":"/host","tmpfs":{"size":"1","mode":511}}));
            }),
            Box::new(|model, _| {
                model["services"]["pocketbase"]["volumes"][0]["target"] = json!("/var/lib");
            }),
            Box::new(|model, _| {
                model["services"]["kicad-cli"]["volumes"][0]["source"] =
                    json!("/var/run/docker.sock");
            }),
            Box::new(|model, _| {
                model["services"]["kicad-cli"]["volumes"][0]["target"] = json!("/workspace");
            }),
            Box::new(|model, _| {
                model["services"]["kicad-cli"]["volumes"][0]["read_only"] = json!(false);
            }),
            Box::new(|model, expected| {
                model["services"]["kicad-cli"]["volumes"][0]["source"] =
                    json!(compose_path(expected.fixture_dir.parent().unwrap()));
            }),
            Box::new(|model, expected| {
                model["services"]["pocketbase"]["build"]["context"] =
                    json!(compose_path(expected.fixture_dir.parent().unwrap()));
            }),
        ];
        for (index, mutation) in mutations.into_iter().enumerate() {
            let (_temporary, mut expected, mut model) = setup();
            mutation(&mut model, &mut expected);
            assert!(
                validate_compose_policy(&model.to_string(), &expected).is_err(),
                "accepted mutation {index}"
            );
        }
    }

    #[test]
    fn every_service_requires_exact_enforced_memory_and_pid_limits() {
        for service in ["pocketbase", "minio", "test-runner", "kicad-cli"] {
            for key in ["mem_limit", "pids_limit"] {
                for mutation in ["missing", "zero", "wrong", "oversized"] {
                    let (_temporary, expected, mut model) = setup();
                    let object = model["services"][service].as_object_mut().unwrap();
                    match (key, mutation) {
                        (_, "missing") => {
                            object.remove(key);
                        }
                        ("mem_limit", "zero") => object[key] = json!("0"),
                        ("mem_limit", "wrong") => object[key] = json!("1"),
                        ("mem_limit", "oversized") => object[key] = json!("17179869184"),
                        ("pids_limit", "zero") => object[key] = json!(0),
                        ("pids_limit", "wrong") => object[key] = json!(1),
                        ("pids_limit", "oversized") => object[key] = json!(1_000_000),
                        _ => unreachable!(),
                    }
                    assert!(
                        validate_compose_policy(&model.to_string(), &expected).is_err(),
                        "accepted {service}.{key} mutation {mutation}"
                    );
                }
            }
            let (_temporary, expected, mut model) = setup();
            model["services"][service]
                .as_object_mut()
                .unwrap()
                .insert("cpus".into(), json!("1"));
            assert!(
                validate_compose_policy(&model.to_string(), &expected).is_err(),
                "accepted unexpected resource setting for {service}"
            );
        }
    }

    #[test]
    fn every_service_requires_exact_tmpfs_targets_sizes_and_modes() {
        for service in ["pocketbase", "minio", "test-runner", "kicad-cli"] {
            let (_temporary, expected, base) = setup();
            let tmpfs_index = base["services"][service]["volumes"]
                .as_array()
                .unwrap()
                .iter()
                .position(|mount| mount["type"] == "tmpfs")
                .unwrap();
            for mutation in [
                "missing",
                "zero-size",
                "wrong-size",
                "oversized",
                "zero-mode",
                "wrong-mode",
                "unexpected-key",
            ] {
                let mut model = base.clone();
                let volumes = model["services"][service]["volumes"]
                    .as_array_mut()
                    .unwrap();
                match mutation {
                    "missing" => {
                        volumes.remove(tmpfs_index);
                    }
                    "zero-size" => volumes[tmpfs_index]["tmpfs"]["size"] = json!("0"),
                    "wrong-size" => volumes[tmpfs_index]["tmpfs"]["size"] = json!("1"),
                    "oversized" => volumes[tmpfs_index]["tmpfs"]["size"] = json!("17179869184"),
                    "zero-mode" => volumes[tmpfs_index]["tmpfs"]["mode"] = json!(0),
                    "wrong-mode" => volumes[tmpfs_index]["tmpfs"]["mode"] = json!(511),
                    "unexpected-key" => {
                        volumes[tmpfs_index]["tmpfs"]
                            .as_object_mut()
                            .unwrap()
                            .insert("copy_up".into(), json!(true));
                    }
                    _ => unreachable!(),
                }
                assert!(
                    validate_compose_policy(&model.to_string(), &expected).is_err(),
                    "accepted {service} tmpfs mutation {mutation}"
                );
            }
            let mut model = base;
            model["services"][service]["volumes"]
                .as_array_mut()
                .unwrap()
                .push(
                    json!({"type":"tmpfs","target":"/surprise","tmpfs":{"size":"1","mode":1023}}),
                );
            assert!(
                validate_compose_policy(&model.to_string(), &expected).is_err(),
                "accepted unexpected tmpfs for {service}"
            );
        }
    }

    #[test]
    fn overlapping_allowlisted_sources_are_rejected() {
        let (_temporary, mut expected, mut model) = setup();
        let nested = expected.fixture_dir.join("artifacts");
        fs::create_dir(&nested).unwrap();
        expected.artifact_dir = nested.clone();
        model["services"]["kicad-cli"]["volumes"][1]["source"] = json!(compose_path(&nested));
        assert!(validate_compose_policy(&model.to_string(), &expected).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn linked_allowlisted_source_is_rejected() {
        use std::os::unix::fs::symlink;
        let (_temporary, mut expected, mut model) = setup();
        let real = expected.fixture_dir.clone();
        let linked = real.parent().unwrap().join("linked-fixture");
        symlink(&real, &linked).unwrap();
        expected.fixture_dir = linked.clone();
        model["services"]["kicad-cli"]["volumes"][0]["source"] = json!(compose_path(&linked));
        assert!(validate_compose_policy(&model.to_string(), &expected).is_err());
    }
}
