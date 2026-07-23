use afk_lab::{
    CleanupAction, ComposeCommand, EvidenceManifest, FailureStage, LabError, LabResult, LaneResult,
    LaneStatus, RunPlan, RuntimeIsolationEvidence, VersionsLock, artifact_integrity_for_bytes,
    choose_compose, cleanup_action, enumerate_artifacts, expected_service_resource_bounds,
    expected_service_tmpfs_bounds, expected_service_writable_targets, host_secret_path_exists,
    initial_lanes, protected_sentinels, redact, reject_runtime_state_errors, service_is_healthy,
    validate_compose_policy, validate_digest, validate_minio_output, validate_pocketbase_output,
    validate_runner_output, validate_runtime_isolation_output, verify_artifact_manifest,
    write_json_new,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(20 * 60);

#[derive(Debug)]
enum Cli {
    Test(Options),
    Cleanup { run_id: String },
}

#[derive(Debug)]
struct Options {
    static_only: bool,
    preserve_on_failure: bool,
    run_id: Option<String>,
    inject_failure: Option<FailureStage>,
}

#[derive(Debug)]
struct Invocation {
    success: bool,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

struct RunState {
    lanes: BTreeMap<String, LaneResult>,
    compose: Option<ComposeCommand>,
    resources_may_exist: bool,
    protected_before: Option<BTreeMap<String, String>>,
    run_canary_before: Option<String>,
    host_secret_exists: Option<bool>,
    container_secret_probe_passed: bool,
}

impl RunState {
    fn new() -> Self {
        Self {
            lanes: initial_lanes(),
            compose: None,
            resources_may_exist: false,
            protected_before: None,
            run_canary_before: None,
            host_secret_exists: None,
            container_secret_probe_passed: false,
        }
    }

    fn pass(&mut self, lane: &str, detail: &str, log: Option<&str>) {
        self.lanes
            .insert(lane.into(), lane_result(LaneStatus::Passed, detail, log));
    }

    fn fail(&mut self, lane: &str, detail: &str, log: Option<&str>) -> LabError {
        self.lanes
            .insert(lane.into(), lane_result(LaneStatus::Failed, detail, log));
        LabError(detail.into())
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> LabResult<()> {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|error| LabError(format!("cannot resolve repository: {error}")))?;
    let lock = VersionsLock::load(
        &repository.join("infra/versions.lock"),
        &repository.join("rust-toolchain.toml"),
    )?;
    match parse_cli(std::env::args().skip(1).collect())? {
        Cli::Test(options) => run_test(&repository, &lock, options),
        Cli::Cleanup { run_id } => cleanup_existing(&repository, &lock, &run_id),
    }
}

fn parse_cli(args: Vec<String>) -> LabResult<Cli> {
    match args.first().map(String::as_str) {
        Some("test-afk") => parse_test_options(&args[1..]).map(Cli::Test),
        Some("cleanup-afk") => {
            if args.len() != 3 || args[1] != "--run-id" {
                return Err(LabError("usage: cargo xtask cleanup-afk --run-id ID".into()));
            }
            Ok(Cli::Cleanup { run_id: args[2].clone() })
        }
        _ => Err(LabError(
            "usage: cargo xtask test-afk [--static-only] [--preserve-on-failure] [--run-id ID] [--inject-failure STAGE]\n       cargo xtask cleanup-afk --run-id ID"
                .into(),
        )),
    }
}

fn parse_test_options(args: &[String]) -> LabResult<Options> {
    let mut options = Options {
        static_only: false,
        preserve_on_failure: false,
        run_id: None,
        inject_failure: None,
    };
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        if !seen.insert(flag.to_owned()) {
            return Err(LabError(format!("duplicate option `{flag}`")));
        }
        match flag {
            "--static-only" => options.static_only = true,
            "--preserve-on-failure" => options.preserve_on_failure = true,
            "--run-id" | "--inject-failure" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| LabError(format!("{flag} requires a value")))?;
                if flag == "--run-id" {
                    options.run_id = Some(value.clone());
                } else {
                    options.inject_failure = Some(FailureStage::parse(value)?);
                }
            }
            _ => return Err(LabError(format!("unknown test-afk option `{flag}`"))),
        }
        index += 1;
    }
    if options.static_only && options.preserve_on_failure {
        return Err(LabError(
            "--preserve-on-failure is incompatible with --static-only".into(),
        ));
    }
    if options.static_only
        && options
            .inject_failure
            .is_some_and(|stage| !stage.compatible_with_static())
    {
        return Err(LabError(
            "selected failure injection requires a runtime stack".into(),
        ));
    }
    Ok(options)
}

fn run_test(repository: &Path, lock: &VersionsLock, options: Options) -> LabResult<()> {
    let plan = RunPlan::create(repository, options.run_id.as_deref(), lock)?;
    plan.prepare()?;
    let mut state = RunState::new();
    let execution = execute_guarded(repository, &plan, lock, &options, &mut state);
    let mut error = execution.err();

    let protected_after = protected_sentinels(repository);
    let canary_after = plan.run_canary_hash();
    if error.is_none() {
        match (&state.protected_before, &protected_after) {
            (Some(before), Ok(after)) if before == after => {}
            (Some(_), Ok(_)) => {
                error = Some(state.fail("orchestration", "protected input sentinel changed", None))
            }
            (_, Err(after_error)) => {
                error = Some(state.fail(
                    "orchestration",
                    &format!("final protected input sentinel failed: {after_error}"),
                    None,
                ))
            }
            (None, _) => {
                error = Some(state.fail(
                    "orchestration",
                    "initial protected input sentinel was unavailable",
                    None,
                ))
            }
        }
        match (&state.run_canary_before, &canary_after) {
            (Some(before), Ok(after)) if before == after => {}
            (Some(_), Ok(_)) => {
                error = Some(state.fail("orchestration", "scoped run canary changed", None))
            }
            (_, Err(after_error)) => {
                error = Some(state.fail(
                    "orchestration",
                    &format!("final scoped run canary failed: {after_error}"),
                    None,
                ))
            }
            (None, _) => {
                error = Some(state.fail(
                    "orchestration",
                    "initial scoped run canary was unavailable",
                    None,
                ))
            }
        }
    }

    let action = cleanup_action(error.is_none(), options.preserve_on_failure);
    let teardown = finalize_resources(repository, &plan, lock, &state, action);
    if let Err(teardown_error) = teardown {
        error = Some(state.fail(
            "orchestration",
            &format!("teardown failed: {teardown_error}"),
            Some("teardown.json"),
        ));
    }
    if options.inject_failure == Some(FailureStage::TeardownFailure) && error.is_none() {
        error = Some(state.fail(
            "orchestration",
            "injected teardown failure after verified cleanup",
            Some("teardown.json"),
        ));
    }
    if options.inject_failure == Some(FailureStage::EvidenceWrite) && error.is_none() {
        error = Some(state.fail("orchestration", "injected evidence write failure", None));
    }

    let isolation = isolation_manifest(&state, &protected_after, &canary_after);
    if let Err(isolation_error) =
        write_json_new(&plan.artifact_dir.join("isolation.json"), &isolation)
    {
        error = Some(state.fail(
            "orchestration",
            &format!("isolation evidence write failed: {isolation_error}"),
            None,
        ));
    }
    if error.is_none() {
        state.pass(
            "orchestration",
            "execution, evidence preparation, and teardown passed",
            Some("teardown.json"),
        );
    }
    let overall = if error.is_none() {
        LaneStatus::Passed
    } else {
        LaneStatus::Failed
    };
    let bundle = write_evidence_bundle(&plan, overall, &state.lanes);
    if let Err(bundle_error) = bundle {
        let _ = plan.remove_state();
        return Err(LabError(format!(
            "final evidence bundle failed after cleanup: {bundle_error}"
        )));
    }

    match error {
        None => {
            println!(
                "AFK run {} passed; evidence: {}",
                plan.run_id,
                plan.artifact_dir.display()
            );
            Ok(())
        }
        Some(error) => {
            if action == CleanupAction::PreserveRun {
                eprintln!("preserved failed run {}", plan.run_id);
                eprintln!("cleanup: {}", plan.cleanup_command());
                eprintln!("state: {}", plan.state_dir.display());
            }
            Err(LabError(redact(
                &error.to_string(),
                &plan.credentials.secret_values(),
            )))
        }
    }
}

fn execute_guarded(
    repository: &Path,
    plan: &RunPlan,
    lock: &VersionsLock,
    options: &Options,
    state: &mut RunState,
) -> LabResult<()> {
    state.protected_before = Some(protected_sentinels(repository).map_err(|error| {
        state.fail(
            "static-policy",
            &format!("initial protected input sentinel failed: {error}"),
            None,
        )
    })?);
    state.run_canary_before = Some(plan.run_canary_hash().map_err(|error| {
        state.fail(
            "static-policy",
            &format!("initial scoped run canary failed: {error}"),
            None,
        )
    })?);
    state.host_secret_exists = Some(host_secret_path_exists(repository).map_err(|error| {
        state.fail(
            "static-policy",
            &format!("host secret metadata probe failed: {error}"),
            None,
        )
    })?);

    let compose = if options.inject_failure == Some(FailureStage::MissingCompose) {
        return Err(state.fail("static-policy", "injected missing Compose executable", None));
    } else {
        detect_compose().map_err(|error| state.fail("static-policy", &error.to_string(), None))?
    };
    state.compose = Some(compose.clone());
    ensure_project_absent(plan)
        .map_err(|error| state.fail("static-policy", &error.to_string(), None))?;

    let normalized = compose_call(
        repository,
        plan,
        lock,
        &compose,
        ["config", "--format", "json"],
    )
    .map_err(|error| state.fail("static-policy", &error.to_string(), None))?;
    write_log(plan, "compose-config.log", &normalized)
        .map_err(|error| state.fail("static-policy", &error.to_string(), None))?;
    let policy = validate_compose_policy(&normalized.stdout, &plan.policy_expectation(lock))
        .map_err(|error| {
            state.fail(
                "static-policy",
                &error.to_string(),
                Some("compose-config.log"),
            )
        })?;
    write_json_new(&plan.artifact_dir.join("isolation-policy.json"), &policy)
        .map_err(|error| state.fail("static-policy", &error.to_string(), None))?;
    validate_contract_files(repository, lock)
        .map_err(|error| state.fail("static-policy", &error.to_string(), None))?;
    state.pass(
        "static-policy",
        "typed lock, Dockerfile, and normalized Compose allowlist passed",
        Some("isolation-policy.json"),
    );
    if options.inject_failure == Some(FailureStage::AfterPlan) {
        return Err(state.fail(
            "orchestration",
            "injected failure after plan validation",
            None,
        ));
    }
    if options.static_only {
        return Ok(());
    }

    state.resources_may_exist = true;
    let build = compose_call(
        repository,
        plan,
        lock,
        &compose,
        ["build", "pocketbase", "minio", "test-runner"],
    )
    .map_err(|error| state.fail("foundation-stack", &error.to_string(), None))?;
    write_log(plan, "compose-build.log", &build)
        .map_err(|error| state.fail("foundation-stack", &error.to_string(), None))?;
    let up = compose_call(
        repository,
        plan,
        lock,
        &compose,
        ["up", "-d", "--wait", "pocketbase", "minio"],
    )
    .map_err(|error| {
        state.fail(
            "foundation-stack",
            &error.to_string(),
            Some("compose-build.log"),
        )
    })?;
    write_log(plan, "compose-up.log", &up)
        .map_err(|error| state.fail("foundation-stack", &error.to_string(), None))?;
    if options.inject_failure == Some(FailureStage::AfterStart) {
        return Err(state.fail(
            "foundation-stack",
            "injected failure after service start",
            Some("compose-up.log"),
        ));
    }
    if options.inject_failure == Some(FailureStage::CommandTimeout) {
        let timeout = invoke_with_timeout(&mut long_sleep_command(), Duration::from_millis(100));
        if !timeout.timed_out {
            return Err(state.fail("orchestration", "timeout injection did not time out", None));
        }
        return Err(state.fail("orchestration", "injected bounded command timeout", None));
    }

    let ps = compose_call(repository, plan, lock, &compose, ["ps"])
        .map_err(|error| state.fail("foundation-stack", &error.to_string(), None))?;
    write_log(plan, "compose-ps.log", &ps)
        .map_err(|error| state.fail("foundation-stack", &error.to_string(), None))?;
    let health_state = if options.inject_failure == Some(FailureStage::HealthMismatch) {
        "unhealthy"
    } else {
        &ps.stdout
    };
    service_is_healthy(ps.success, health_state).map_err(|error| {
        state.fail(
            "foundation-stack",
            &error.to_string(),
            Some("compose-ps.log"),
        )
    })?;

    let mut runtime_isolation = BTreeMap::<String, RuntimeIsolationEvidence>::new();
    for service in ["pocketbase", "minio"] {
        let evidence = capture_runtime_isolation(repository, plan, lock, &compose, service, true)
            .map_err(|error| state.fail("foundation-stack", &error.to_string(), None))?;
        runtime_isolation.insert(service.into(), evidence);
    }

    let connectivity = compose_call(repository, plan, lock, &compose, [
        "run", "--rm", "--no-deps", "test-runner", "sh", "-ec",
        "wget -q -O /dev/null http://pocketbase:8090/api/health; wget -q -O /dev/null http://minio:9000/minio/health/live; test ! -e /workspace/secrets.env; test ! -e /secrets.env; printf 'pocketbase:8090 reachable\\nminio:9000 reachable\\nHOST_SECRET_PATHS_ABSENT\\n'",
    ]).map_err(|error| state.fail("foundation-stack", &error.to_string(), None))?;
    write_log(plan, "internal-connectivity.log", &connectivity)
        .map_err(|error| state.fail("foundation-stack", &error.to_string(), None))?;
    state.container_secret_probe_passed =
        output_text(&connectivity).contains("HOST_SECRET_PATHS_ABSENT");

    let pocketbase = compose_call(
        repository,
        plan,
        lock,
        &compose,
        ["run", "--rm", "--no-deps", "pocketbase", "--version"],
    )
    .map_err(|error| state.fail("foundation-stack", &error.to_string(), None))?;
    write_log(plan, "pocketbase-version.log", &pocketbase)
        .map_err(|error| state.fail("foundation-stack", &error.to_string(), None))?;
    let pocketbase_text = if options.inject_failure == Some(FailureStage::VersionMismatch) {
        "pocketbase version 0.0.0"
    } else {
        &output_text(&pocketbase)
    };
    validate_pocketbase_output(pocketbase_text, lock).map_err(|error| {
        state.fail(
            "foundation-stack",
            &error.to_string(),
            Some("pocketbase-version.log"),
        )
    })?;
    let minio = compose_call(
        repository,
        plan,
        lock,
        &compose,
        ["run", "--rm", "--no-deps", "minio", "--version"],
    )
    .map_err(|error| state.fail("foundation-stack", &error.to_string(), None))?;
    write_log(plan, "minio-version.log", &minio)
        .map_err(|error| state.fail("foundation-stack", &error.to_string(), None))?;
    validate_minio_output(&output_text(&minio), lock).map_err(|error| {
        state.fail(
            "foundation-stack",
            &error.to_string(),
            Some("minio-version.log"),
        )
    })?;
    state.pass(
        "foundation-stack",
        "PocketBase and MinIO exact versions, health, internal endpoints, and writable isolated state passed",
        Some("runtime-isolation-pocketbase.log"),
    );

    let runner_isolation =
        capture_runtime_isolation(repository, plan, lock, &compose, "test-runner", false)
            .map_err(|error| state.fail("test-runner", &error.to_string(), None))?;
    runtime_isolation.insert("test-runner".into(), runner_isolation);

    let runner = compose_call(repository, plan, lock, &compose, [
        "run", "--rm", "--no-deps", "test-runner", "sh", "-ec",
        "id; rustc --version; cargo --version; node --version; npm --version; tauri --version; command -v WebKitWebDriver; command -v Xvfb; dpkg-query -W -f='${binary:Package} ${Version}\\n' webkit2gtk-driver libwebkit2gtk-4.1-0 xvfb; test ! -e /workspace/secrets.env; test ! -e /secrets.env; printf 'HOST_SECRET_PATHS_ABSENT\\n'",
    ]).map_err(|error| state.fail("test-runner", &error.to_string(), None))?;
    write_log(plan, "runner-versions.log", &runner)
        .map_err(|error| state.fail("test-runner", &error.to_string(), None))?;
    validate_runner_output(&output_text(&runner), lock).map_err(|error| {
        state.fail(
            "test-runner",
            &error.to_string(),
            Some("runner-versions.log"),
        )
    })?;
    state.pass(
        "test-runner",
        "exact non-root Rust/Node/npm/Tauri/WebKitGTK browser-test runner and writable isolated state passed",
        Some("runtime-isolation-test-runner.log"),
    );

    if runtime_isolation.len() != 3 {
        return Err(state.fail(
            "orchestration",
            "runtime isolation evidence is incomplete",
            None,
        ));
    }
    write_json_new(
        &plan.artifact_dir.join("runtime-isolation.json"),
        &json!({
            "schema": 1,
            "run_id": plan.run_id,
            "services": runtime_isolation,
        }),
    )
    .map_err(|error| state.fail("orchestration", &error.to_string(), None))?;

    let environment = runtime_environment(plan, lock, &compose, &pocketbase, &minio, &runner)?;
    write_json_new(&plan.artifact_dir.join("environment.json"), &environment)
        .map_err(|error| state.fail("orchestration", &error.to_string(), None))?;
    write_json_new(
        &plan.artifact_dir.join("ports.json"),
        &json!({
            "scope": "compose-project-internal-network", "published_to_host": false,
            "pocketbase": "pocketbase:8090", "minio": "minio:9000"
        }),
    )
    .map_err(|error| state.fail("orchestration", &error.to_string(), None))?;
    Ok(())
}

fn detect_compose() -> LabResult<ComposeCommand> {
    let plugin = invoke(Command::new("docker").args(["compose", "version"])).success;
    let standalone = invoke(Command::new("docker-compose").arg("version")).success;
    choose_compose(plugin, standalone)
}

fn ensure_project_absent(plan: &RunPlan) -> LabResult<()> {
    for mut command in project_resource_queries(&plan.compose_project) {
        let output = invoke(&mut command);
        if !output.success {
            return Err(LabError(format!(
                "cannot inspect existing scoped resources: {}",
                output_text(&output)
            )));
        }
        if !output.stdout.trim().is_empty() {
            return Err(LabError(format!(
                "Compose project `{}` already has Docker resources",
                plan.compose_project
            )));
        }
    }
    Ok(())
}

fn project_resource_queries(project: &str) -> Vec<Command> {
    let label = format!("label=com.docker.compose.project={project}");
    let mut containers = Command::new("docker");
    containers.args(["ps", "-aq", "--filter", &label]);
    let mut volumes = Command::new("docker");
    volumes.args(["volume", "ls", "-q", "--filter", &label]);
    let mut networks = Command::new("docker");
    networks.args(["network", "ls", "-q", "--filter", &label]);
    vec![containers, volumes, networks]
}

fn base_compose_args(plan: &RunPlan, compose: &ComposeCommand) -> Vec<String> {
    let mut args = compose.prefix_args.clone();
    args.extend([
        "-f".into(),
        "infra/compose.yaml".into(),
        "-f".into(),
        "infra/compose.test.yaml".into(),
        "-p".into(),
        plan.compose_project.clone(),
    ]);
    args
}

fn compose_call<const N: usize>(
    repository: &Path,
    plan: &RunPlan,
    lock: &VersionsLock,
    compose: &ComposeCommand,
    suffix: [&str; N],
) -> LabResult<Invocation> {
    compose_call_vec(
        repository,
        plan,
        lock,
        compose,
        suffix.into_iter().map(str::to_owned).collect(),
    )
}

fn compose_call_vec(
    repository: &Path,
    plan: &RunPlan,
    lock: &VersionsLock,
    compose: &ComposeCommand,
    suffix: Vec<String>,
) -> LabResult<Invocation> {
    let mut args = base_compose_args(plan, compose);
    args.extend(suffix.iter().cloned());
    let output = invoke(
        Command::new(&compose.program)
            .args(&args)
            .envs(plan.compose_environment(lock))
            .current_dir(repository),
    );
    if output.success {
        Ok(output)
    } else {
        let detail = redact(&output_text(&output), &plan.credentials.secret_values());
        Err(LabError(format!(
            "Compose command `{}` failed: {detail}",
            suffix.join(" ")
        )))
    }
}

fn runtime_isolation_probe_script(service: &str, lock: &VersionsLock) -> LabResult<String> {
    let uid = lock.toolchain.runner_uid;
    let gid = lock.toolchain.runner_gid;
    let (memory_bytes, pids) = expected_service_resource_bounds(service)?;
    let tmpfs = expected_service_tmpfs_bounds(service)?;
    let writable = expected_service_writable_targets(service)?;
    let mut script = format!(
        "set -eu; test \"$(id -u)\" = '{uid}'; test \"$(id -g)\" = '{gid}'; printf '%s\\n' 'AFK_ID={uid}:{gid}'; memory=$(cat /sys/fs/cgroup/memory.max); test \"$memory\" = '{memory_bytes}'; printf '%s\\n' 'AFK_CGROUP_MEMORY={memory_bytes}'; pids=$(cat /sys/fs/cgroup/pids.max); test \"$pids\" = '{pids}'; printf '%s\\n' 'AFK_CGROUP_PIDS={pids}'; "
    );
    for (target, (size_bytes, mode)) in &tmpfs {
        let size_kib = size_bytes / 1024;
        script.push_str(&format!(
            "owner=$(stat -c '%u:%g' '{target}'); test \"$owner\" = '0:0'; actual_mode=$(stat -c '%a' '{target}'); test \"$actual_mode\" = '{mode:o}'; options=$(awk -v target='{target}' '$2 == target {{print $4; found=1}} END {{if (!found) exit 1}}' /proc/mounts); case \",$options,\" in *,rw,*) ;; *) exit 71 ;; esac; case \",$options,\" in *,size={size_kib}k,*) ;; *) exit 72 ;; esac; printf '%s\\n' 'AFK_TMPFS={target}|owner=0:0|mode={mode:o}|size_bytes={size_bytes}|options=rw,size|mode_source=stat'; "
        ));
    }
    for target in writable {
        let is_mount_root = tmpfs.contains_key(&target);
        let (owner, mode) = if is_mount_root {
            ("0:0".to_owned(), 0o1777)
        } else {
            script.push_str(&format!("mkdir -p '{target}'; chmod 700 '{target}'; "));
            (format!("{uid}:{gid}"), 0o700)
        };
        script.push_str(&format!(
            "owner=$(stat -c '%u:%g' '{target}'); test \"$owner\" = '{owner}'; actual_mode=$(stat -c '%a' '{target}'); test \"$actual_mode\" = '{mode:o}'; canary='{target}/.afk-write-canary'; test ! -e \"$canary\"; umask 077; printf '%s\\n' 'afk-canary' > \"$canary\"; test \"$(cat \"$canary\")\" = 'afk-canary'; rm \"$canary\"; test ! -e \"$canary\"; printf '%s\\n' 'AFK_WRITABLE={target}|owner={owner}|mode={mode:o}|canary=create-read-delete'; "
        ));
    }
    Ok(script)
}

fn capture_runtime_isolation(
    repository: &Path,
    plan: &RunPlan,
    lock: &VersionsLock,
    compose: &ComposeCommand,
    service: &str,
    running: bool,
) -> LabResult<RuntimeIsolationEvidence> {
    let script = runtime_isolation_probe_script(service, lock)?;
    let mut suffix = if running {
        vec!["exec".into(), "-T".into(), service.into()]
    } else {
        vec![
            "run".into(),
            "--rm".into(),
            "--no-deps".into(),
            service.into(),
        ]
    };
    suffix.extend(["sh".into(), "-ec".into(), script]);
    let result = compose_call_vec(repository, plan, lock, compose, suffix)?;
    let log = format!("runtime-isolation-{service}.log");
    write_log(plan, &log, &result)?;
    validate_runtime_isolation_output(&output_text(&result), service, lock)
}

fn invoke(command: &mut Command) -> Invocation {
    invoke_with_timeout(command, COMMAND_TIMEOUT)
}

fn invoke_with_timeout(command: &mut Command, timeout: Duration) -> Invocation {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    configure_process_group(command);
    let process_tree = match ProcessTree::new() {
        Ok(process_tree) => process_tree,
        Err(error) => {
            return Invocation {
                success: false,
                stdout: String::new(),
                stderr: error.to_string(),
                timed_out: false,
            };
        }
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Invocation {
                success: false,
                stdout: String::new(),
                stderr: error.to_string(),
                timed_out: false,
            };
        }
    };
    let child_id = child.id();
    if let Err(error) = process_tree.attach(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Invocation {
            success: false,
            stdout: String::new(),
            stderr: error.to_string(),
            timed_out: false,
        };
    }
    let mut stdout = child.stdout.take().expect("stdout was piped");
    let mut stderr = child.stderr.take().expect("stderr was piped");
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });
    let started = Instant::now();
    let (success, timed_out, detail) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status.success(), false, None),
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                process_tree.terminate(child_id);
                let _ = child.kill();
                let _ = child.wait();
                break (
                    false,
                    true,
                    Some(format!(
                        "command timed out after {} seconds",
                        timeout.as_secs_f64()
                    )),
                );
            }
            Err(error) => {
                process_tree.terminate(child_id);
                let _ = child.kill();
                let _ = child.wait();
                break (
                    false,
                    false,
                    Some(format!("cannot wait for command: {error}")),
                );
            }
        }
    };
    let stdout = String::from_utf8_lossy(&stdout_reader.join().unwrap_or_default()).into_owned();
    let mut stderr =
        String::from_utf8_lossy(&stderr_reader.join().unwrap_or_default()).into_owned();
    if let Some(detail) = detail {
        if !stderr.is_empty() && !stderr.ends_with('\n') {
            stderr.push('\n');
        }
        stderr.push_str(&detail);
    }
    Invocation {
        success,
        stdout,
        stderr,
        timed_out,
    }
}

#[cfg(windows)]
struct ProcessTree {
    job: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl ProcessTree {
    fn new() -> LabResult<Self> {
        use std::mem::{size_of, zeroed};
        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        };
        let job = unsafe {
            windows_sys::Win32::System::JobObjects::CreateJobObjectW(
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if job.is_null() {
            return Err(LabError(format!(
                "cannot create Windows process Job Object: {}",
                std::io::Error::last_os_error()
            )));
        }
        let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&information as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
            return Err(LabError(format!(
                "cannot configure Windows process Job Object: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(Self { job })
    }

    fn attach(&self, child: &std::process::Child) -> LabResult<()> {
        use std::os::windows::io::AsRawHandle;
        let attached = unsafe {
            windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
                self.job,
                child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
            )
        };
        if attached == 0 {
            return Err(LabError(format!(
                "cannot attach child to Windows process Job Object: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    fn terminate(&self, pid: u32) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.job) };
    }
}

#[cfg(unix)]
struct ProcessTree;

#[cfg(unix)]
impl ProcessTree {
    fn new() -> LabResult<Self> {
        Ok(Self)
    }

    fn attach(&self, _child: &std::process::Child) -> LabResult<()> {
        Ok(())
    }

    fn terminate(&self, pid: u32) {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
fn configure_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0000_0200);
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

fn output_text(output: &Invocation) -> String {
    format!("{}{}", output.stdout, output.stderr)
        .trim()
        .to_owned()
}

fn write_log(plan: &RunPlan, name: &str, output: &Invocation) -> LabResult<()> {
    let raw = output_text(output);
    reject_runtime_state_errors(&raw)?;
    let content = redact(&raw, &plan.credentials.secret_values());
    write_text_new(&plan.artifact_dir.join(name), &format!("{content}\n"))
}

fn runtime_environment(
    plan: &RunPlan,
    lock: &VersionsLock,
    compose: &ComposeCommand,
    pocketbase: &Invocation,
    minio: &Invocation,
    runner: &Invocation,
) -> LabResult<Value> {
    let docker = invoke(Command::new("docker").args([
        "version",
        "--format",
        "{{.Client.Version}}/{{.Server.Version}}",
    ]));
    if !docker.success {
        return Err(LabError("Docker version inventory failed".into()));
    }
    let compose_version = invoke(
        Command::new(&compose.program)
            .args(&compose.prefix_args)
            .arg("version"),
    );
    if !compose_version.success {
        return Err(LabError("Compose version inventory failed".into()));
    }
    let mut images = serde_json::Map::new();
    for (name, reference) in [
        ("pocketbase", plan.images.pocketbase.as_str()),
        ("minio", plan.images.minio.as_str()),
        ("test-runner", plan.images.test_runner.as_str()),
    ] {
        let inspect = invoke(
            Command::new("docker").args(["image", "inspect", reference, "--format", "{{.Id}}"]),
        );
        let content_id = inspect.stdout.trim();
        if !inspect.success {
            return Err(LabError(format!(
                "image inspection failed for `{reference}`"
            )));
        }
        validate_digest(content_id, &format!("images.{name}.content_id"))?;
        images.insert(
            name.into(),
            json!({"used_reference": reference, "content_id": content_id}),
        );
    }
    Ok(json!({
        "schema": 2, "run_id": plan.run_id, "lock": lock,
        "tools": {
            "docker": output_text(&docker), "compose": output_text(&compose_version),
            "pocketbase": output_text(pocketbase), "minio": output_text(minio),
            "runner": output_text(runner),
        },
        "images": images,
    }))
}

fn validate_contract_files(repository: &Path, _lock: &VersionsLock) -> LabResult<()> {
    validate_owned_runtime_scope(repository)?;
    let compose = fs::read_to_string(repository.join("infra/compose.yaml"))?;
    for variable in [
        "AFK_POCKETBASE_IMAGE",
        "AFK_MINIO_IMAGE",
        "AFK_TEST_RUNNER_IMAGE",
        "AFK_BUSYBOX_IMAGE",
        "AFK_RUST_IMAGE",
        "AFK_POCKETBASE_VERSION",
        "AFK_POCKETBASE_URL",
        "AFK_POCKETBASE_SHA256",
        "AFK_MINIO_COMMIT",
        "AFK_MINIO_SHORT_COMMIT",
        "AFK_MINIO_VERSION",
        "AFK_MINIO_BUILD_VERSION",
        "AFK_MINIO_COPYRIGHT_YEAR",
        "AFK_MINIO_SOURCE_URL",
        "AFK_MINIO_SOURCE_SHA256",
        "AFK_GO_URL",
        "AFK_GO_SHA256",
        "AFK_RUST_TOOLCHAIN_DIR",
        "AFK_DEBIAN_SNAPSHOT",
        "AFK_NODE_VERSION",
        "AFK_NODE_URL",
        "AFK_NODE_SHA256",
        "AFK_TAURI_VERSION",
        "AFK_TAURI_URL",
        "AFK_TAURI_SHA512",
        "AFK_TAURI_LINUX_X64_GNU_URL",
        "AFK_TAURI_LINUX_X64_GNU_SHA512",
        "AFK_RUNNER_UID",
        "AFK_RUNNER_GID",
    ] {
        if !compose.contains(&format!("${{{variable}}}")) {
            return Err(LabError(format!(
                "Compose does not consume lock variable `{variable}`"
            )));
        }
    }
    if compose.contains("AFK_REPO_ROOT")
        || compose.contains("/workspace")
        || compose.contains("ports:")
    {
        return Err(LabError(
            "Compose contract contains a repository bind, workspace path, or host ports".into(),
        ));
    }
    for dockerfile in ["pocketbase", "minio", "test-runner"] {
        let text =
            fs::read_to_string(repository.join(format!("infra/docker/{dockerfile}/Dockerfile")))?;
        if text.contains("@sha256:") || text.contains("=0.38.2") || text.contains("=24.18.0") {
            return Err(LabError(format!(
                "{dockerfile} Dockerfile duplicates a locked version/digest"
            )));
        }
        if dockerfile == "test-runner" {
            validate_test_runner_dockerfile(&text)?;
        }
    }
    Ok(())
}

fn validate_test_runner_dockerfile(text: &str) -> LabResult<()> {
    let required = [
        "ARG TAURI_CLI_LINUX_X64_GNU_URL",
        "ARG TAURI_CLI_LINUX_X64_GNU_SHA512",
        "ADD ${TAURI_CLI_LINUX_X64_GNU_URL} /tmp/tauri-cli-linux-x64-gnu.tgz",
        "echo \"${TAURI_CLI_LINUX_X64_GNU_SHA512}  /tmp/tauri-cli-linux-x64-gnu.tgz\" | sha512sum -c -",
        "${global_root}/@tauri-apps/cli/tauri.js",
        "${global_root}/@tauri-apps/cli-linux-x64-gnu/cli.linux-x64-gnu.node",
        "require('${global_root}/@tauri-apps/cli/package.json').version",
        "require('${global_root}/@tauri-apps/cli-linux-x64-gnu/package.json').version",
        "tauri --version | grep -Fqx \"tauri-cli ${TAURI_CLI_VERSION}\"",
    ];
    for fragment in required {
        if text.matches(fragment).count() != 1 {
            return Err(LabError(format!(
                "test-runner Dockerfile must contain one exact `{fragment}` contract"
            )));
        }
    }

    let logical = dockerfile_logical_lines(text);
    let installs = logical
        .iter()
        .filter(|line| line.contains("npm install"))
        .collect::<Vec<_>>();
    if installs.len() != 1 {
        return Err(LabError(
            "test-runner must contain one deterministic npm install command".into(),
        ));
    }
    let install = installs[0];
    for required in [
        "--global",
        "--offline",
        "--ignore-scripts",
        "--no-audit",
        "--no-fund",
        "--cache /tmp/afk-npm-cache",
        "--userconfig /dev/null",
        "\"/tmp/tauri-cli.tgz\" \"/tmp/tauri-cli-linux-x64-gnu.tgz\"",
    ] {
        if !install.contains(required) {
            return Err(LabError(format!(
                "test-runner deterministic npm install is missing `{required}`"
            )));
        }
    }
    Ok(())
}

fn dockerfile_logical_lines(text: &str) -> Vec<String> {
    let mut logical = Vec::new();
    let mut current = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let continued = line.ends_with('\\');
        let segment = line.strip_suffix('\\').unwrap_or(line).trim_end();
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(segment);
        if !continued {
            logical.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        logical.push(current);
    }
    logical
}

fn validate_owned_runtime_scope(repository: &Path) -> LabResult<()> {
    for relative in [".github", "infra", "tooling/afk", "tooling/xtask"] {
        scan_runtime_text(&repository.join(relative))?;
    }
    Ok(())
}

fn scan_runtime_text(path: &Path) -> LabResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(LabError(format!(
            "runtime contract path is linked: `{}`",
            path.display()
        )));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            scan_runtime_text(&entry?.path())?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(LabError(format!(
            "runtime contract path is not regular: `{}`",
            path.display()
        )));
    }
    let text = fs::read_to_string(path)?;
    if let Some(violation) = owned_automation_violation(&text) {
        return Err(LabError(format!(
            "runtime/tooling implementation contains forbidden automation `{violation}` in `{}`",
            path.display()
        )));
    }
    Ok(())
}

fn owned_automation_violation(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let forbidden_fragments = [
        ["fixtures/", "kicad-10/gui-config"].concat(),
        ["fixtures/", "kicad-10/afk-smoke"].concat(),
        ["ki", "cad-gui-smoke"].concat(),
        ["ubuntu", "_gui"].concat(),
        ["ppa:", "ki", "cad"].concat(),
        ["add-apt-", "repository"].concat(),
        ["open", "box"].concat(),
        ["image", "magick"].concat(),
        ["systemd-", "coredump"].concat(),
    ];
    if let Some(fragment) = forbidden_fragments
        .iter()
        .find(|fragment| lower.contains(fragment.as_str()))
    {
        return Some(fragment.clone());
    }

    let native = [
        ["ki", "cad"].concat(),
        ["ki", "cad-cli"].concat(),
        ["pcb", "new"].concat(),
    ];
    let scripting = [
        ["py", "thon"].concat(),
        ["py", "thon3"].concat(),
        ["py", "qt"].concat(),
        ["toml", "lib"].concat(),
    ];
    automation_tokens(text).find_map(|token| {
        let lower = token.to_ascii_lowercase();
        let automation_prefix = ["ki", "cad_"].concat();
        if native.contains(&lower)
            || scripting.contains(&lower)
            || lower.starts_with(&automation_prefix)
        {
            Some(token.to_owned())
        } else {
            None
        }
    })
}

fn automation_tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character: char| {
        !(character.is_ascii_alphanumeric() || character == '_' || character == '-')
    })
    .filter(|token| !token.is_empty())
}

fn finalize_resources(
    repository: &Path,
    plan: &RunPlan,
    lock: &VersionsLock,
    state: &RunState,
    action: CleanupAction,
) -> LabResult<()> {
    if action == CleanupAction::PreserveRun {
        return write_json_new(
            &plan.artifact_dir.join("teardown.json"),
            &json!({
                "status": "preserved", "run_id": plan.run_id, "cleanup_command": plan.cleanup_command(),
                "secret_state_on_disk": false, "run_canary_retained": true,
            }),
        );
    }
    let result = if state.resources_may_exist {
        let compose = state
            .compose
            .as_ref()
            .ok_or_else(|| LabError("resources may exist but Compose is unavailable".into()))?;
        cleanup_docker(repository, plan, lock, compose)
    } else {
        Ok("no Docker resources were started".into())
    };
    match result {
        Ok(detail) => {
            write_json_new(
                &plan.artifact_dir.join("teardown.json"),
                &json!({
                    "status": "removed", "run_id": plan.run_id, "detail": detail,
                    "scoped_resources_remaining": false, "secret_state_on_disk": false,
                }),
            )?;
            plan.remove_state()?;
            Ok(())
        }
        Err(error) => {
            write_json_new(
                &plan.artifact_dir.join("teardown.json"),
                &json!({
                    "status": "failed", "run_id": plan.run_id,
                    "detail": redact(&error.to_string(), &plan.credentials.secret_values()),
                    "cleanup_command": plan.cleanup_command(), "scoped_resources_may_remain": true,
                }),
            )?;
            Err(error)
        }
    }
}

fn cleanup_docker(
    repository: &Path,
    plan: &RunPlan,
    lock: &VersionsLock,
    compose: &ComposeCommand,
) -> LabResult<String> {
    let down = compose_call(
        repository,
        plan,
        lock,
        compose,
        ["down", "--volumes", "--remove-orphans"],
    )?;
    let mut details = vec![output_text(&down)];
    for image in [
        &plan.images.pocketbase,
        &plan.images.minio,
        &plan.images.test_runner,
    ] {
        let inspect =
            invoke(Command::new("docker").args(["image", "inspect", image, "--format", "{{.Id}}"]));
        if inspect.success {
            let remove = invoke(Command::new("docker").args(["image", "rm", image]));
            if !remove.success {
                return Err(LabError(format!(
                    "cannot remove scoped image `{image}`: {}",
                    output_text(&remove)
                )));
            }
            details.push(output_text(&remove));
        }
    }
    for mut query in project_resource_queries(&plan.compose_project) {
        let output = invoke(&mut query);
        if !output.success || !output.stdout.trim().is_empty() {
            return Err(LabError(format!(
                "scoped Docker resources remain for `{}`: {}",
                plan.compose_project,
                output_text(&output)
            )));
        }
    }
    Ok(details
        .into_iter()
        .filter(|detail| !detail.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}

fn cleanup_existing(repository: &Path, lock: &VersionsLock, run_id: &str) -> LabResult<()> {
    let plan = RunPlan::open_existing(repository, run_id, lock)?;
    let compose = detect_compose()?;
    cleanup_docker(repository, &plan, lock, &compose)?;
    plan.remove_state()?;
    println!("AFK run {run_id} cleaned; state and scoped Docker resources removed");
    Ok(())
}

fn isolation_manifest(
    state: &RunState,
    after: &LabResult<BTreeMap<String, String>>,
    canary_after: &LabResult<String>,
) -> Value {
    let protected_after = after.as_ref().ok();
    let canary_after = canary_after.as_ref().ok();
    json!({
        "schema": 2,
        "protected_inputs": {
            "before": state.protected_before, "after": protected_after,
            "unchanged": state.protected_before.as_ref().zip(protected_after).is_some_and(|(before, after)| before == after),
            "scope": ["database", "symbols", "footprints", "3d-models", "fixtures/import"],
        },
        "scoped_run_canary": {
            "before": state.run_canary_before, "after": canary_after,
            "unchanged": state.run_canary_before.as_ref().zip(canary_after).is_some_and(|(before, after)| before == after),
            "proves": "only that the run-scoped state canary was not modified while present",
        },
        "host_secret_isolation": {
            "host_path_exists_by_metadata_only": state.host_secret_exists,
            "container_secret_path_probe_passed": state.container_secret_probe_passed,
            "proof": "normalized exact bind allowlist plus runtime absence probes; secret contents were never read",
        }
    })
}

fn write_evidence_bundle(
    plan: &RunPlan,
    overall: LaneStatus,
    lanes: &BTreeMap<String, LaneResult>,
) -> LabResult<()> {
    let mut artifacts = enumerate_artifacts(&plan.artifact_dir)?;
    let failures = lanes
        .values()
        .filter(|lane| lane.status == LaneStatus::Failed)
        .count();
    let skipped = lanes
        .values()
        .filter(|lane| lane.status == LaneStatus::NotRun)
        .count();
    let mut cases = String::new();
    for (name, lane) in lanes {
        cases.push_str(&format!("  <testcase name=\"{}\">", xml(name)));
        match lane.status {
            LaneStatus::Failed => {
                cases.push_str(&format!("<failure message=\"{}\"/>", xml(&lane.detail)))
            }
            LaneStatus::NotRun => {
                cases.push_str(&format!("<skipped message=\"{}\"/>", xml(&lane.detail)))
            }
            LaneStatus::Passed => {}
        }
        cases.push_str("</testcase>\n");
    }
    let junit = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"afk-lab\" tests=\"{}\" failures=\"{failures}\" skipped=\"{skipped}\">\n{cases}</testsuite>\n",
        lanes.len()
    );
    artifacts.insert(
        "results.xml".into(),
        artifact_integrity_for_bytes(junit.as_bytes()),
    );
    let manifest = EvidenceManifest {
        schema: 3,
        run_id: plan.run_id.clone(),
        overall,
        lanes: lanes.clone(),
        artifacts: artifacts.clone(),
        unhashed_self: "evidence.json".into(),
    };
    let junit_path = plan.artifact_dir.join("results.xml");
    let evidence_path = plan.artifact_dir.join("evidence.json");
    let junit_pending = plan.artifact_dir.join("results.xml.pending");
    let evidence_pending = plan.artifact_dir.join("evidence.json.pending");
    if junit_path.exists()
        || evidence_path.exists()
        || junit_pending.exists()
        || evidence_pending.exists()
    {
        return Err(LabError(
            "refusing to replace an existing or incomplete final evidence bundle".into(),
        ));
    }

    let staged = (|| {
        write_text_new(&junit_pending, &junit)?;
        write_json_new(&evidence_pending, &manifest)?;
        Ok(())
    })();
    if let Err(error) = staged {
        let _ = fs::remove_file(&junit_pending);
        let _ = fs::remove_file(&evidence_pending);
        return Err(error);
    }

    if let Err(error) = fs::rename(&junit_pending, &junit_path) {
        let _ = fs::remove_file(&junit_pending);
        let _ = fs::remove_file(&evidence_pending);
        return Err(LabError(format!(
            "cannot finalize `{}`: {error}",
            junit_path.display()
        )));
    }
    if let Err(error) = fs::rename(&evidence_pending, &evidence_path) {
        // A half-finalized bundle must never survive as apparently authoritative.
        let _ = fs::remove_file(&junit_path);
        let _ = fs::remove_file(&evidence_pending);
        return Err(LabError(format!(
            "cannot finalize `{}`: {error}",
            evidence_path.display()
        )));
    }
    if let Err(error) = verify_artifact_manifest(&plan.artifact_dir, &artifacts) {
        let _ = fs::remove_file(&evidence_path);
        let _ = fs::remove_file(&junit_path);
        return Err(error);
    }
    Ok(())
}

fn lane_result(status: LaneStatus, detail: &str, log: Option<&str>) -> LaneResult {
    LaneResult {
        status,
        detail: detail.into(),
        log: log.map(str::to_owned),
    }
}

fn write_text_new(path: &Path, text: &str) -> LabResult<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| LabError(format!("cannot create `{}`: {error}", path.display())))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(windows)]
fn long_sleep_command() -> Command {
    let mut command = Command::new("powershell");
    command.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
    command
}

#[cfg(not(windows))]
fn long_sleep_command() -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 30"]);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn cli_rejects_duplicate_unknown_and_incompatible_options() {
        assert!(
            parse_cli(vec![
                "test-afk".into(),
                "--run-id".into(),
                "one-one".into(),
                "--run-id".into(),
                "two-two".into()
            ])
            .is_err()
        );
        assert!(parse_cli(vec!["test-afk".into(), "--wat".into()]).is_err());
        assert!(
            parse_cli(vec![
                "test-afk".into(),
                "--static-only".into(),
                "--inject-failure".into(),
                "after-start".into()
            ])
            .is_err()
        );
        assert!(
            parse_cli(vec![
                "cleanup-afk".into(),
                "--run-id".into(),
                "safe-id".into(),
                "extra".into()
            ])
            .is_err()
        );
    }

    #[cfg(windows)]
    fn output_command() -> Command {
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-Command",
            "[Console]::Out.Write(('o' * 131072)); [Console]::Error.Write(('e' * 131072))",
        ]);
        command
    }

    #[cfg(not(windows))]
    fn output_command() -> Command {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "head -c 131072 /dev/zero | tr '\\0' o; head -c 131072 /dev/zero | tr '\\0' e >&2",
        ]);
        command
    }

    #[test]
    fn bounded_runner_drains_stdout_and_stderr_without_deadlock() {
        let output = invoke_with_timeout(&mut output_command(), Duration::from_secs(10));
        assert!(output.success, "{}", output_text(&output));
        assert_eq!(output.stdout.len(), 131072);
        assert_eq!(output.stderr.len(), 131072);
    }

    #[test]
    fn bounded_runner_kills_a_timed_out_child() {
        let output = invoke_with_timeout(&mut long_sleep_command(), Duration::from_millis(100));
        assert!(!output.success);
        assert!(output.timed_out);
        assert!(output.stderr.contains("command timed out"));
    }

    #[cfg(windows)]
    fn grandchild_command() -> Command {
        let mut command = Command::new("powershell");
        command.args(["-NoProfile", "-Command", "$p=Start-Process powershell -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 30' -PassThru; [Console]::Out.WriteLine($p.Id); Start-Sleep -Seconds 30"]);
        command
    }

    #[cfg(not(windows))]
    fn grandchild_command() -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & echo $!; wait"]);
        command
    }

    #[test]
    fn timeout_terminates_descendant_process_tree() {
        let output = invoke_with_timeout(&mut grandchild_command(), Duration::from_secs(2));
        assert!(output.timed_out);
        let pid = output
            .stdout
            .lines()
            .next()
            .expect("grandchild command must report its child PID before timing out")
            .trim();
        #[cfg(windows)]
        {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let probe = Command::new("powershell")
                    .args([
                        "-NoProfile",
                        "-Command",
                        &format!(
                            "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 1 }}"
                        ),
                    ])
                    .status()
                    .unwrap();
                if probe.success() {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "grandchild {pid} survived taskkill /T"
                );
                thread::sleep(Duration::from_millis(100));
            }
        }
        #[cfg(unix)]
        {
            let probe = Command::new("sh")
                .args(["-c", &format!("! kill -0 {pid} 2>/dev/null")])
                .status()
                .unwrap();
            assert!(
                probe.success(),
                "grandchild {pid} survived process-group kill"
            );
        }
    }

    #[test]
    fn evidence_bundle_fails_closed_when_artifact_directory_is_missing() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let lock = VersionsLock::load(
            &repository.join("infra/versions.lock"),
            &repository.join("rust-toolchain.toml"),
        )
        .unwrap();
        let temporary = tempdir().unwrap();
        let plan = RunPlan::create(temporary.path(), Some("missing-artifacts"), &lock).unwrap();
        assert!(write_evidence_bundle(&plan, LaneStatus::Failed, &initial_lanes()).is_err());
    }

    #[test]
    fn checked_in_lock_compose_and_dockerfiles_match() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let lock = VersionsLock::load(
            &repository.join("infra/versions.lock"),
            &repository.join("rust-toolchain.toml"),
        )
        .unwrap();
        validate_contract_files(&repository, &lock).unwrap();
    }

    #[test]
    fn owned_runtime_scope_rejects_forbidden_dependencies() {
        let temporary = tempdir().unwrap();
        let safe = temporary.path().join("safe.txt");
        fs::write(&safe, "cargo xtask test-afk --static-only\n").unwrap();
        let forbidden = [
            ["py", "thon"].concat(),
            ["py", "thon3"].concat(),
            ["py", "qt"].concat(),
            ["toml", "lib"].concat(),
        ];
        scan_runtime_text(temporary.path(), &forbidden).unwrap();
        fs::write(&safe, ["py", "thon3", " -c pass\n"].concat()).unwrap();
        assert!(scan_runtime_text(temporary.path(), &forbidden).is_err());
    }
}
