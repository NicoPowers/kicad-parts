use afk_lab::{
    CleanupAction, ComposeCommand, EvidenceManifest, FailureStage, LabError, LabResult, LaneResult,
    LaneStatus, RunPlan, RuntimeIsolationEvidence, VersionsLock, artifact_integrity_for_bytes,
    choose_compose, cleanup_action, enumerate_artifacts, expected_service_resource_bounds,
    expected_service_tmpfs_bounds, expected_service_writable_targets, host_secret_path_exists,
    initial_lanes, protected_sentinels, redact, reject_runtime_state_errors, service_is_healthy,
    validate_compose_policy, validate_digest, validate_kicad_output, validate_minio_output,
    validate_pocketbase_output, validate_runner_output, validate_runtime_isolation_output,
    verify_artifact_manifest, write_json_new,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use yaml_rust2::{Yaml, YamlLoader};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(20 * 60);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum WorkflowContractCategory {
    ActionPin,
    AlwaysUpload,
    Availability,
    CliVersion,
    DiagnosticLog,
    DpkgAssertion,
    DowngradePermission,
    ExecutablePreflight,
    Exporter,
    GuiLog,
    HashProof,
    InstallSpec,
    Isolation,
    Junit,
    LaneEvidence,
    LockReference,
    Ppa,
    PackageManifest,
    PrivateToolchain,
    Screenshot,
    StockPath,
    TauriArtifact,
    ToolchainProbe,
    UploadPath,
    Window,
    Xvfb,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum WorkflowStep {
    Checkout,
    Export,
    Install,
    Smoke,
    EnsureEvidence,
    Upload,
}

impl WorkflowStep {
    const fn index(self) -> usize {
        match self {
            Self::Checkout => 0,
            Self::Export => 1,
            Self::Install => 2,
            Self::Smoke => 3,
            Self::EnsureEvidence => 4,
            Self::Upload => 5,
        }
    }

    const fn name(self) -> Option<&'static str> {
        match self {
            Self::Checkout => None,
            Self::Export => Some("Load exact Ubuntu GUI package contract from versions.lock"),
            Self::Install => Some("Install exact KiCad and pinned Tauri/X11 runner"),
            Self::Smoke => Some("Launch isolated KiCad GUI smoke and write lane evidence"),
            Self::EnsureEvidence => Some("Ensure machine-readable failure evidence exists"),
            Self::Upload => Some("Upload diagnostics"),
        }
    }
}

const WORKFLOW_STEPS: &[WorkflowStep] = &[
    WorkflowStep::Checkout,
    WorkflowStep::Export,
    WorkflowStep::Install,
    WorkflowStep::Smoke,
    WorkflowStep::EnsureEvidence,
    WorkflowStep::Upload,
];

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum WorkflowOrderGroup {
    InstallPipeline,
    SmokePipeline,
}

#[derive(Clone, Copy, Debug)]
enum RunMatchKind {
    Line,
    Token,
}

#[derive(Clone, Copy, Debug)]
struct RunLocation {
    step: WorkflowStep,
    occurrences: usize,
    order: Option<(WorkflowOrderGroup, u16)>,
}

#[derive(Clone, Copy, Debug)]
enum WorkflowContractLocator {
    Uses {
        step: WorkflowStep,
    },
    If {
        step: WorkflowStep,
    },
    With {
        step: WorkflowStep,
        key: &'static str,
    },
    JobField {
        key: &'static str,
    },
    JobEnv {
        key: &'static str,
    },
    StepEnv {
        step: WorkflowStep,
        key: &'static str,
    },
    TriggerPath,
    Run {
        kind: RunMatchKind,
        locations: &'static [RunLocation],
    },
}

#[derive(Clone, Copy, Debug)]
struct WorkflowContractEntry {
    category: WorkflowContractCategory,
    locator: WorkflowContractLocator,
    text: &'static str,
}

macro_rules! field_contract {
    ($category:ident, $locator:expr, $text:expr) => {
        WorkflowContractEntry {
            category: WorkflowContractCategory::$category,
            locator: $locator,
            text: $text,
        }
    };
}

macro_rules! run_contract {
    ($category:ident, $kind:ident, $text:expr, $(($step:ident, $occurrences:expr, $order:expr)),+ $(,)?) => {
        WorkflowContractEntry {
            category: WorkflowContractCategory::$category,
            locator: WorkflowContractLocator::Run {
                kind: RunMatchKind::$kind,
                locations: &[$(RunLocation {
                    step: WorkflowStep::$step,
                    occurrences: $occurrences,
                    order: $order,
                }),+],
            },
            text: $text,
        }
    };
}

const WORKFLOW_CONTRACT: &[WorkflowContractEntry] = &[
    field_contract!(
        LockReference,
        WorkflowContractLocator::TriggerPath,
        "infra/versions.lock"
    ),
    field_contract!(
        LockReference,
        WorkflowContractLocator::JobField { key: "runs-on" },
        "ubuntu-24.04"
    ),
    field_contract!(
        LockReference,
        WorkflowContractLocator::JobEnv {
            key: "RUST_VERSION"
        },
        "1.85.1"
    ),
    field_contract!(
        LockReference,
        WorkflowContractLocator::JobEnv {
            key: "NODE_VERSION"
        },
        "24.18.0"
    ),
    field_contract!(
        LockReference,
        WorkflowContractLocator::JobEnv { key: "NPM_VERSION" },
        "11.16.0"
    ),
    field_contract!(
        LockReference,
        WorkflowContractLocator::JobEnv { key: "NODE_URL" },
        "https://nodejs.org/dist/v24.18.0/node-v24.18.0-linux-x64.tar.xz"
    ),
    field_contract!(
        LockReference,
        WorkflowContractLocator::JobEnv { key: "NODE_SHA256" },
        "55aa7153f9d88f28d765fcdad5ae6945b5c0f98a36881703817e4c450fa76742"
    ),
    field_contract!(
        LockReference,
        WorkflowContractLocator::JobEnv {
            key: "TAURI_VERSION"
        },
        "2.11.4"
    ),
    field_contract!(
        LockReference,
        WorkflowContractLocator::JobEnv { key: "TAURI_URL" },
        "https://registry.npmjs.org/@tauri-apps/cli/-/cli-2.11.4.tgz"
    ),
    field_contract!(
        LockReference,
        WorkflowContractLocator::JobEnv {
            key: "TAURI_SHA512"
        },
        "47cc46b4ca70c9eb5ac12aa6f6460eb8c984aa48546ef714cbc9f468d5c8c689652812c4494bb97f8171fbae218004989b51fe8d25aaea3096eb3a939c7ea1a9"
    ),
    field_contract!(
        ActionPin,
        WorkflowContractLocator::Uses {
            step: WorkflowStep::Checkout
        },
        "actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683"
    ),
    run_contract!(
        Exporter,
        Line,
        "cargo xtask export-github-env >> \"$GITHUB_ENV\"",
        (Export, 1, None)
    ),
    run_contract!(
        Ppa,
        Line,
        "sudo add-apt-repository --yes \"$KICAD_PPA\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 10)))
    ),
    run_contract!(
        Availability,
        Line,
        "apt-cache madison \"$KICAD_PACKAGE\" | awk '{print $3}' | grep -Fqx -- \"$KICAD_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 20)))
    ),
    run_contract!(
        Availability,
        Line,
        "apt-cache madison \"$KICAD_SYMBOLS_PACKAGE\" | awk '{print $3}' | grep -Fqx -- \"$KICAD_SYMBOLS_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 21)))
    ),
    run_contract!(
        Availability,
        Line,
        "apt-cache madison \"$KICAD_FOOTPRINTS_PACKAGE\" | awk '{print $3}' | grep -Fqx -- \"$KICAD_FOOTPRINTS_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 22)))
    ),
    run_contract!(
        Availability,
        Line,
        "apt-cache madison \"$KICAD_PACKAGES3D_PACKAGE\" | awk '{print $3}' | grep -Fqx -- \"$KICAD_PACKAGES3D_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 23)))
    ),
    run_contract!(
        Availability,
        Line,
        "apt-cache madison \"$WEBKIT_LIBRARY_PACKAGE\" | awk '{print $3}' | grep -Fqx -- \"$WEBKIT_LIBRARY_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 24)))
    ),
    run_contract!(
        Availability,
        Line,
        "apt-cache madison \"$WEBKIT_DRIVER_PACKAGE\" | awk '{print $3}' | grep -Fqx -- \"$WEBKIT_DRIVER_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 25)))
    ),
    run_contract!(
        Availability,
        Line,
        "apt-cache madison \"$XVFB_PACKAGE\" | awk '{print $3}' | grep -Fqx -- \"$XVFB_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 26)))
    ),
    run_contract!(
        Availability,
        Line,
        "apt-cache madison \"$X11_UTILS_PACKAGE\" | awk '{print $3}' | grep -Fqx -- \"$X11_UTILS_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 27)))
    ),
    run_contract!(
        Availability,
        Line,
        "apt-cache madison \"$IMAGEMAGICK_PACKAGE\" | awk '{print $3}' | grep -Fqx -- \"$IMAGEMAGICK_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 28)))
    ),
    run_contract!(
        DowngradePermission,
        Line,
        "sudo apt-get install --yes --allow-downgrades",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 29)))
    ),
    run_contract!(
        InstallSpec,
        Token,
        "\"$KICAD_SPEC\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 30)))
    ),
    run_contract!(
        InstallSpec,
        Token,
        "\"$KICAD_SYMBOLS_SPEC\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 31)))
    ),
    run_contract!(
        InstallSpec,
        Token,
        "\"$KICAD_FOOTPRINTS_SPEC\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 32)))
    ),
    run_contract!(
        InstallSpec,
        Token,
        "\"$KICAD_PACKAGES3D_SPEC\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 33)))
    ),
    run_contract!(
        InstallSpec,
        Token,
        "\"$WEBKIT_LIBRARY_SPEC\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 34)))
    ),
    run_contract!(
        InstallSpec,
        Token,
        "\"$WEBKIT_DRIVER_SPEC\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 35)))
    ),
    run_contract!(
        InstallSpec,
        Token,
        "\"$XVFB_SPEC\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 36)))
    ),
    run_contract!(
        InstallSpec,
        Token,
        "\"$X11_UTILS_SPEC\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 37)))
    ),
    run_contract!(
        InstallSpec,
        Token,
        "\"$IMAGEMAGICK_SPEC\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 38)))
    ),
    run_contract!(
        DpkgAssertion,
        Line,
        "test \"$(dpkg-query -W -f='${Version}' \"$KICAD_PACKAGE\")\" = \"$KICAD_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 40))),
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 20)))
    ),
    run_contract!(
        DpkgAssertion,
        Line,
        "test \"$(dpkg-query -W -f='${Version}' \"$KICAD_SYMBOLS_PACKAGE\")\" = \"$KICAD_SYMBOLS_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 41)))
    ),
    run_contract!(
        DpkgAssertion,
        Line,
        "test \"$(dpkg-query -W -f='${Version}' \"$KICAD_FOOTPRINTS_PACKAGE\")\" = \"$KICAD_FOOTPRINTS_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 42)))
    ),
    run_contract!(
        DpkgAssertion,
        Line,
        "test \"$(dpkg-query -W -f='${Version}' \"$KICAD_PACKAGES3D_PACKAGE\")\" = \"$KICAD_PACKAGES3D_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 43)))
    ),
    run_contract!(
        DpkgAssertion,
        Line,
        "test \"$(dpkg-query -W -f='${Version}' \"$WEBKIT_LIBRARY_PACKAGE\")\" = \"$WEBKIT_LIBRARY_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 44)))
    ),
    run_contract!(
        DpkgAssertion,
        Line,
        "test \"$(dpkg-query -W -f='${Version}' \"$WEBKIT_DRIVER_PACKAGE\")\" = \"$WEBKIT_DRIVER_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 45)))
    ),
    run_contract!(
        DpkgAssertion,
        Line,
        "test \"$(dpkg-query -W -f='${Version}' \"$XVFB_PACKAGE\")\" = \"$XVFB_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 46)))
    ),
    run_contract!(
        DpkgAssertion,
        Line,
        "test \"$(dpkg-query -W -f='${Version}' \"$X11_UTILS_PACKAGE\")\" = \"$X11_UTILS_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 47)))
    ),
    run_contract!(
        DpkgAssertion,
        Line,
        "test \"$(dpkg-query -W -f='${Version}' \"$IMAGEMAGICK_PACKAGE\")\" = \"$IMAGEMAGICK_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 48)))
    ),
    run_contract!(
        ExecutablePreflight,
        Line,
        "test \"$(command -v kicad-cli)\" = /usr/bin/kicad-cli",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 50)))
    ),
    run_contract!(
        ExecutablePreflight,
        Line,
        "test \"$(command -v pcbnew)\" = /usr/bin/pcbnew",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 51)))
    ),
    run_contract!(
        ExecutablePreflight,
        Line,
        "test \"$(command -v Xvfb)\" = /usr/bin/Xvfb",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 52)))
    ),
    run_contract!(
        ExecutablePreflight,
        Line,
        "test \"$(command -v xwininfo)\" = /usr/bin/xwininfo",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 53)))
    ),
    run_contract!(
        ExecutablePreflight,
        Line,
        "test \"$(command -v import)\" = /usr/bin/import",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 54)))
    ),
    run_contract!(
        PrivateToolchain,
        Line,
        "toolchain_prefix=\"$RUNNER_TEMP/kicad-gui-toolchain-$NODE_VERSION-$TAURI_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 60)))
    ),
    run_contract!(
        PrivateToolchain,
        Line,
        "test ! -e \"$toolchain_prefix\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 61)))
    ),
    run_contract!(
        PrivateToolchain,
        Line,
        "mkdir \"$toolchain_prefix\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 62)))
    ),
    run_contract!(
        PrivateToolchain,
        Line,
        "tar -xJf /tmp/node.tar.xz --strip-components=1 -C \"$toolchain_prefix\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 63)))
    ),
    run_contract!(
        PrivateToolchain,
        Line,
        "export PATH=\"$toolchain_prefix/bin:$PATH\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 64)))
    ),
    run_contract!(
        TauriArtifact,
        Line,
        "curl --fail --location --silent --show-error \"$TAURI_LINUX_X64_GNU_URL\" --output /tmp/tauri-cli-linux-x64-gnu.tgz",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 65)))
    ),
    run_contract!(
        TauriArtifact,
        Line,
        "echo \"$TAURI_LINUX_X64_GNU_SHA512  /tmp/tauri-cli-linux-x64-gnu.tgz\" | sha512sum -c -",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 66)))
    ),
    run_contract!(
        DiagnosticLog,
        Line,
        "artifact_dir=\"$GITHUB_WORKSPACE/.afk/gui/artifacts\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 67)))
    ),
    run_contract!(
        DiagnosticLog,
        Line,
        "npm_logs=\"$artifact_dir/npm-logs\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 68)))
    ),
    run_contract!(
        PrivateToolchain,
        Line,
        "npm_home=\"$RUNNER_TEMP/kicad-gui-npm-home\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 69)))
    ),
    run_contract!(
        PrivateToolchain,
        Line,
        "env HOME=\"$npm_home\" NPM_CONFIG_USERCONFIG=/dev/null \"$toolchain_prefix/bin/npm\" install",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 70)))
    ),
    run_contract!(
        PrivateToolchain,
        Line,
        "--global --prefix \"$toolchain_prefix\" --offline --omit=optional --ignore-scripts",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 71)))
    ),
    run_contract!(
        DiagnosticLog,
        Line,
        "--loglevel verbose --logs-dir \"$npm_logs\" --audit=false --fund=false",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 72)))
    ),
    run_contract!(
        PrivateToolchain,
        Line,
        "--update-notifier=false /tmp/tauri-cli.tgz /tmp/tauri-cli-linux-x64-gnu.tgz",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 73)))
    ),
    run_contract!(
        DiagnosticLog,
        Line,
        "2>&1 | tee \"$artifact_dir/npm-tauri-install.log\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 74)))
    ),
    run_contract!(
        PrivateToolchain,
        Line,
        "echo \"$toolchain_prefix/bin\" >> \"$GITHUB_PATH\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 75)))
    ),
    run_contract!(
        ToolchainProbe,
        Line,
        "test \"$(rustc --version | awk '{print $2}')\" = \"$RUST_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 80)))
    ),
    run_contract!(
        ToolchainProbe,
        Line,
        "test \"$(\"$toolchain_prefix/bin/node\" --version)\" = \"v$NODE_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 81)))
    ),
    run_contract!(
        ToolchainProbe,
        Line,
        "test \"$(\"$toolchain_prefix/bin/npm\" --version)\" = \"$NPM_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 82)))
    ),
    run_contract!(
        ToolchainProbe,
        Line,
        "test \"$(\"$toolchain_prefix/bin/tauri\" --version)\" = \"tauri-cli $TAURI_VERSION\"",
        (Install, 1, Some((WorkflowOrderGroup::InstallPipeline, 83)))
    ),
    field_contract!(
        Isolation,
        WorkflowContractLocator::StepEnv {
            step: WorkflowStep::Smoke,
            key: "HOME"
        },
        "${{ github.workspace }}/.afk/gui/home"
    ),
    field_contract!(
        Isolation,
        WorkflowContractLocator::StepEnv {
            step: WorkflowStep::Smoke,
            key: "XDG_CACHE_HOME"
        },
        "${{ github.workspace }}/.afk/gui/xdg/cache"
    ),
    field_contract!(
        Isolation,
        WorkflowContractLocator::StepEnv {
            step: WorkflowStep::Smoke,
            key: "XDG_CONFIG_HOME"
        },
        "${{ github.workspace }}/.afk/gui/xdg/config"
    ),
    field_contract!(
        Isolation,
        WorkflowContractLocator::StepEnv {
            step: WorkflowStep::Smoke,
            key: "XDG_DATA_HOME"
        },
        "${{ github.workspace }}/.afk/gui/xdg/data"
    ),
    field_contract!(
        Isolation,
        WorkflowContractLocator::StepEnv {
            step: WorkflowStep::Smoke,
            key: "XDG_STATE_HOME"
        },
        "${{ github.workspace }}/.afk/gui/xdg/state"
    ),
    field_contract!(
        Isolation,
        WorkflowContractLocator::StepEnv {
            step: WorkflowStep::Smoke,
            key: "KICAD_CONFIG_HOME"
        },
        "${{ github.workspace }}/.afk/gui/kicad/config"
    ),
    field_contract!(
        Isolation,
        WorkflowContractLocator::StepEnv {
            step: WorkflowStep::Smoke,
            key: "DISPLAY"
        },
        ":99"
    ),
    run_contract!(
        HashProof,
        Line,
        "sha256sum \"$fixture\" > \"$artifact_dir/source.before.sha256\"",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 10)))
    ),
    run_contract!(
        PackageManifest,
        Line,
        "dpkg-query -W \"$KICAD_PACKAGE\" \"$KICAD_SYMBOLS_PACKAGE\" \"$KICAD_FOOTPRINTS_PACKAGE\" \"$KICAD_PACKAGES3D_PACKAGE\" \"$WEBKIT_LIBRARY_PACKAGE\" \"$WEBKIT_DRIVER_PACKAGE\" \"$XVFB_PACKAGE\" \"$X11_UTILS_PACKAGE\" \"$IMAGEMAGICK_PACKAGE\"",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 15)))
    ),
    run_contract!(
        CliVersion,
        Line,
        "kicad_upstream_version=\"${KICAD_VERSION%%~*}\"",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 21)))
    ),
    run_contract!(
        StockPath,
        Line,
        "test -f \"$STOCK_SYMBOL\"",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 30)))
    ),
    run_contract!(
        StockPath,
        Line,
        "test -f \"$STOCK_FOOTPRINT\"",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 31)))
    ),
    run_contract!(
        StockPath,
        Line,
        "test -f \"$STOCK_3D_MODEL\"",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 32)))
    ),
    run_contract!(
        CliVersion,
        Line,
        "kicad-cli version --format about | tee \"$artifact_dir/kicad-version.txt\"",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 33)))
    ),
    run_contract!(
        CliVersion,
        Line,
        "grep -Fqx -- \"Version: $kicad_upstream_version-$KICAD_VERSION, release build\" \"$artifact_dir/kicad-version.txt\"",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 34)))
    ),
    run_contract!(
        Xvfb,
        Token,
        "Xvfb :99 -screen 0 1280x800x24",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 40)))
    ),
    run_contract!(
        GuiLog,
        Token,
        "> \"$artifact_dir/xvfb.log\" 2>&1",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 41)))
    ),
    run_contract!(
        GuiLog,
        Token,
        "> \"$artifact_dir/kicad-gui.log\" 2>&1",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 42)))
    ),
    run_contract!(
        Window,
        Token,
        "xwininfo -root -tree",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 43)))
    ),
    run_contract!(
        Screenshot,
        Line,
        "import -window root \"$artifact_dir/kicad-gui.png\"",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 50)))
    ),
    run_contract!(
        HashProof,
        Line,
        "sha256sum -c \"$artifact_dir/source.before.sha256\"",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 60)))
    ),
    run_contract!(
        HashProof,
        Line,
        "cmp \"$artifact_dir/source.before.sha256\" \"$artifact_dir/source.after.sha256\"",
        (Smoke, 1, Some((WorkflowOrderGroup::SmokePipeline, 61)))
    ),
    run_contract!(
        Junit,
        Token,
        "\"$artifact_dir/results.xml\"",
        (Smoke, 1, None),
        (EnsureEvidence, 1, None)
    ),
    run_contract!(
        LaneEvidence,
        Token,
        "\"$artifact_dir/lane-result.json\"",
        (Smoke, 1, None),
        (EnsureEvidence, 2, None)
    ),
    field_contract!(
        AlwaysUpload,
        WorkflowContractLocator::If {
            step: WorkflowStep::EnsureEvidence
        },
        "always()"
    ),
    field_contract!(
        ActionPin,
        WorkflowContractLocator::Uses {
            step: WorkflowStep::Upload
        },
        "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02"
    ),
    field_contract!(
        AlwaysUpload,
        WorkflowContractLocator::If {
            step: WorkflowStep::Upload
        },
        "always()"
    ),
    field_contract!(
        AlwaysUpload,
        WorkflowContractLocator::With {
            step: WorkflowStep::Upload,
            key: "if-no-files-found"
        },
        "error"
    ),
    field_contract!(
        UploadPath,
        WorkflowContractLocator::With {
            step: WorkflowStep::Upload,
            key: "path"
        },
        ".afk/gui/artifacts"
    ),
];

fn expected_workflow_category_counts() -> BTreeMap<WorkflowContractCategory, usize> {
    BTreeMap::from([
        (WorkflowContractCategory::ActionPin, 2),
        (WorkflowContractCategory::AlwaysUpload, 3),
        (WorkflowContractCategory::Availability, 9),
        (WorkflowContractCategory::CliVersion, 3),
        (WorkflowContractCategory::DiagnosticLog, 4),
        (WorkflowContractCategory::DpkgAssertion, 9),
        (WorkflowContractCategory::DowngradePermission, 1),
        (WorkflowContractCategory::ExecutablePreflight, 5),
        (WorkflowContractCategory::Exporter, 1),
        (WorkflowContractCategory::GuiLog, 2),
        (WorkflowContractCategory::HashProof, 3),
        (WorkflowContractCategory::InstallSpec, 9),
        (WorkflowContractCategory::Isolation, 7),
        (WorkflowContractCategory::Junit, 1),
        (WorkflowContractCategory::LaneEvidence, 1),
        (WorkflowContractCategory::LockReference, 10),
        (WorkflowContractCategory::Ppa, 1),
        (WorkflowContractCategory::PackageManifest, 1),
        (WorkflowContractCategory::Screenshot, 1),
        (WorkflowContractCategory::StockPath, 3),
        (WorkflowContractCategory::PrivateToolchain, 10),
        (WorkflowContractCategory::TauriArtifact, 2),
        (WorkflowContractCategory::ToolchainProbe, 4),
        (WorkflowContractCategory::UploadPath, 1),
        (WorkflowContractCategory::Window, 1),
        (WorkflowContractCategory::Xvfb, 1),
    ])
}

#[derive(Debug)]
enum Cli {
    Test(Options),
    Cleanup { run_id: String },
    ExportGithubEnv,
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
        Cli::ExportGithubEnv => {
            print!("{}", github_env_export(&lock)?);
            Ok(())
        }
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
        Some("export-github-env") if args.len() == 1 => Ok(Cli::ExportGithubEnv),
        _ => Err(LabError(
            "usage: cargo xtask test-afk [--static-only] [--preserve-on-failure] [--run-id ID] [--inject-failure STAGE]\n       cargo xtask cleanup-afk --run-id ID\n       cargo xtask export-github-env"
                .into(),
        )),
    }
}

fn github_env_export(lock: &VersionsLock) -> LabResult<String> {
    let stock = &lock.ubuntu_gui.stock_packages;
    if stock.len() != 3 {
        return Err(LabError(
            "typed GUI lock must contain exactly three stock packages".into(),
        ));
    }
    let pins = [
        ("KICAD", &lock.ubuntu_gui.kicad_package),
        ("KICAD_SYMBOLS", &stock[0]),
        ("KICAD_FOOTPRINTS", &stock[1]),
        ("KICAD_PACKAGES3D", &stock[2]),
        ("WEBKIT_LIBRARY", &lock.ubuntu_gui.webkit_library),
        ("WEBKIT_DRIVER", &lock.ubuntu_gui.webkit_driver),
        ("XVFB", &lock.ubuntu_gui.xvfb),
        ("X11_UTILS", &lock.ubuntu_gui.x11_utils),
        ("IMAGEMAGICK", &lock.ubuntu_gui.imagemagick),
    ];
    let mut values = BTreeMap::from([
        ("KICAD_PPA".to_owned(), lock.ubuntu_gui.ppa.clone()),
        (
            "STOCK_SYMBOL".to_owned(),
            lock.ubuntu_gui.stock_symbol.clone(),
        ),
        (
            "STOCK_FOOTPRINT".to_owned(),
            lock.ubuntu_gui.stock_footprint.clone(),
        ),
        (
            "STOCK_3D_MODEL".to_owned(),
            lock.ubuntu_gui.stock_3d_model.clone(),
        ),
        (
            "TAURI_LINUX_X64_GNU_SHA512".to_owned(),
            lock.toolchain.tauri_cli_linux_x64_gnu_sha512.clone(),
        ),
        (
            "TAURI_LINUX_X64_GNU_URL".to_owned(),
            lock.toolchain.tauri_cli_linux_x64_gnu_url.clone(),
        ),
    ]);
    for (prefix, pin) in pins {
        values.insert(format!("{prefix}_PACKAGE"), pin.name.clone());
        values.insert(format!("{prefix}_VERSION"), pin.version.clone());
        values.insert(
            format!("{prefix}_SPEC"),
            format!("{}={}", pin.name, pin.version),
        );
    }
    let mut output = String::new();
    for (key, value) in values {
        if value.is_empty() || value.chars().any(|ch| matches!(ch, '\r' | '\n')) {
            return Err(LabError(format!(
                "GitHub environment value `{key}` is empty or multiline"
            )));
        }
        output.push_str(&format!("{key}={value}\n"));
    }
    Ok(output)
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
        "typed lock, workflow, Dockerfile, and normalized Compose allowlist passed",
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
        "wget -q -O /dev/null http://pocketbase:8090/api/health; wget -q -O /dev/null http://minio:9000/minio/health/live; test ! -e /workspace/secrets.env; test ! -e /fixture/secrets.env; test ! -e /secrets.env; printf 'pocketbase:8090 reachable\\nminio:9000 reachable\\nHOST_SECRET_PATHS_ABSENT\\n'",
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
        "id; rustc --version; cargo --version; node --version; npm --version; tauri --version; command -v WebKitWebDriver; command -v Xvfb; dpkg-query -W -f='${binary:Package} ${Version}\\n' webkit2gtk-driver libwebkit2gtk-4.1-0 xvfb; test ! -e /workspace/secrets.env; test ! -e /fixture/secrets.env; test ! -e /secrets.env; printf 'HOST_SECRET_PATHS_ABSENT\\n'",
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
        "exact non-root Rust/Node/npm/Tauri/WebKitGTK/X11 runner and writable isolated state passed",
        Some("runtime-isolation-test-runner.log"),
    );

    let kicad_isolation =
        capture_runtime_isolation(repository, plan, lock, &compose, "kicad-cli", false)
            .map_err(|error| state.fail("kicad-cli", &error.to_string(), None))?;
    runtime_isolation.insert("kicad-cli".into(), kicad_isolation);

    let stock_script = format!(
        "about=$(kicad-cli version --format about); printf '%s\\n' \"$about\"; printf '%s\\n' \"$about\" | grep -Fqx 'Version: {}, release build'; test -f '{}'; printf 'STOCK_SYMBOL_OK {}\\n'; test -f '{}'; printf 'STOCK_FOOTPRINT_OK {}\\n'; test -f '{}'; printf 'STOCK_3D_OK {}\\n'; test ! -e /fixture/secrets.env; test ! -e /workspace/secrets.env; test ! -e /secrets.env; printf 'HOST_SECRET_PATHS_ABSENT\\n'",
        lock.images.kicad.version,
        lock.ubuntu_gui.stock_symbol,
        lock.ubuntu_gui.stock_symbol,
        lock.ubuntu_gui.stock_footprint,
        lock.ubuntu_gui.stock_footprint,
        lock.ubuntu_gui.stock_3d_model,
        lock.ubuntu_gui.stock_3d_model,
    );
    let kicad = compose_call_vec(
        repository,
        plan,
        lock,
        &compose,
        vec![
            "run".into(),
            "--rm".into(),
            "--no-deps".into(),
            "kicad-cli".into(),
            "sh".into(),
            "-ec".into(),
            stock_script,
        ],
    )
    .map_err(|error| state.fail("kicad-cli", &error.to_string(), None))?;
    write_log(plan, "kicad-version-and-stock.log", &kicad)
        .map_err(|error| state.fail("kicad-cli", &error.to_string(), None))?;
    validate_kicad_output(&output_text(&kicad), lock).map_err(|error| {
        state.fail(
            "kicad-cli",
            &error.to_string(),
            Some("kicad-version-and-stock.log"),
        )
    })?;
    let smoke = compose_call(
        repository,
        plan,
        lock,
        &compose,
        [
            "run",
            "--rm",
            "--no-deps",
            "kicad-cli",
            "kicad-cli",
            "pcb",
            "drc",
            "--output",
            "/artifacts/kicad-drc.rpt",
            "/fixture/afk-smoke.kicad_pcb",
        ],
    )
    .map_err(|error| state.fail("kicad-cli", &error.to_string(), None))?;
    write_log(plan, "kicad-cli-smoke.log", &smoke)
        .map_err(|error| state.fail("kicad-cli", &error.to_string(), None))?;
    reject_runtime_state_errors(&output_text(&smoke)).map_err(|error| {
        state.fail("kicad-cli", &error.to_string(), Some("kicad-cli-smoke.log"))
    })?;
    if !output_text(&smoke).contains("Found 0 violations")
        || !output_text(&smoke).contains("Found 0 unconnected items")
    {
        return Err(state.fail(
            "kicad-cli",
            "KiCad DRC output was not exactly clean",
            Some("kicad-cli-smoke.log"),
        ));
    }
    state.pass(
        "kicad-cli",
        "exact KiCad 10 stock inventory, writable isolated state, and clean DRC passed",
        Some("runtime-isolation-kicad-cli.log"),
    );

    if runtime_isolation.len() != 4 {
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

    let environment =
        runtime_environment(plan, lock, &compose, &pocketbase, &minio, &runner, &kicad)?;
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
    kicad: &Invocation,
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
    for (name, reference, locked) in [
        (
            "kicad-cli",
            plan.images.kicad.as_str(),
            Some(lock.images.kicad.digest.as_str()),
        ),
        ("pocketbase", plan.images.pocketbase.as_str(), None),
        ("minio", plan.images.minio.as_str(), None),
        ("test-runner", plan.images.test_runner.as_str(), None),
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
            json!({"used_reference": reference, "locked_digest": locked, "content_id": content_id}),
        );
    }
    Ok(json!({
        "schema": 2, "run_id": plan.run_id, "lock": lock,
        "tools": {
            "docker": output_text(&docker), "compose": output_text(&compose_version),
            "pocketbase": output_text(pocketbase), "minio": output_text(minio),
            "runner": output_text(runner), "kicad": output_text(kicad),
        },
        "images": images,
    }))
}

fn validate_contract_files(repository: &Path, lock: &VersionsLock) -> LabResult<()> {
    validate_no_script_runtime(repository)?;
    let compose = fs::read_to_string(repository.join("infra/compose.yaml"))?;
    for variable in [
        "AFK_POCKETBASE_IMAGE",
        "AFK_MINIO_IMAGE",
        "AFK_TEST_RUNNER_IMAGE",
        "AFK_KICAD_IMAGE",
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
    }
    let workflow = fs::read_to_string(repository.join(".github/workflows/kicad-gui-smoke.yml"))?;
    validate_gui_workflow_contract(&workflow, lock)
}

fn validate_no_script_runtime(repository: &Path) -> LabResult<()> {
    let forbidden = [
        ["py", "thon"].concat(),
        ["py", "thon3"].concat(),
        ["py", "qt"].concat(),
        ["toml", "lib"].concat(),
    ];
    for relative in [
        ".github/workflows/kicad-gui-smoke.yml",
        "infra",
        "tooling/afk",
        "tooling/xtask",
    ] {
        scan_runtime_text(&repository.join(relative), &forbidden)?;
    }
    Ok(())
}

fn scan_runtime_text(path: &Path, forbidden: &[String]) -> LabResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(LabError(format!(
            "runtime contract path is linked: `{}`",
            path.display()
        )));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            scan_runtime_text(&entry?.path(), forbidden)?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(LabError(format!(
            "runtime contract path is not regular: `{}`",
            path.display()
        )));
    }
    let text = fs::read_to_string(path)?.to_ascii_lowercase();
    for needle in forbidden {
        if text.contains(needle) {
            return Err(LabError(format!(
                "runtime/tooling implementation contains forbidden scripting dependency in `{}`",
                path.display()
            )));
        }
    }
    Ok(())
}

fn validate_gui_workflow_contract(workflow: &str, lock: &VersionsLock) -> LabResult<()> {
    let parsed = parse_gui_workflow(workflow)?;
    validate_gui_workflow_value(&parsed, lock)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RunPosition {
    line: usize,
    column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CanonicalShellCommand {
    segments: Vec<String>,
}

const PROTECTED_PROGRAM_DIGESTS: &[(WorkflowStep, &str)] = &[
    (
        WorkflowStep::Export,
        "81b97c2f230abdda0835c5b2d8b423d0b325b1732adfcb87d2483dab496fce82",
    ),
    (
        WorkflowStep::Install,
        "281ad6a30c973fc7ff2b36522e33643ee13d934884de25c4ecd0bff04eb11580",
    ),
    (
        WorkflowStep::Smoke,
        "8e188bed7fb4157a7e7a278f4fd877e9cd9a51ff4b4ed05582c724f4ea0ebac9",
    ),
    (
        WorkflowStep::EnsureEvidence,
        "71be693544f9e2a467ee1892d399ae16cb8128a00633ef37541c95238f44ada0",
    ),
];

fn parse_gui_workflow(workflow: &str) -> LabResult<Yaml> {
    let mut documents = YamlLoader::load_from_str(workflow)
        .map_err(|error| LabError(format!("GUI workflow YAML parse failed: {error}")))?;
    if documents.len() != 1 {
        return Err(LabError(format!(
            "GUI workflow must contain exactly one YAML document, got {}",
            documents.len()
        )));
    }
    Ok(documents.remove(0))
}

fn yaml_key(key: &str) -> Yaml {
    Yaml::String(key.into())
}

fn yaml_field<'a>(node: &'a Yaml, key: &str, location: &str) -> LabResult<&'a Yaml> {
    node.as_hash()
        .ok_or_else(|| LabError(format!("GUI workflow `{location}` must be a mapping")))?
        .get(&yaml_key(key))
        .ok_or_else(|| LabError(format!("GUI workflow is missing `{location}.{key}`")))
}

fn yaml_string<'a>(node: &'a Yaml, location: &str) -> LabResult<&'a str> {
    node.as_str()
        .ok_or_else(|| LabError(format!("GUI workflow `{location}` must be a string")))
}

fn gui_job(workflow: &Yaml) -> LabResult<&Yaml> {
    let jobs = yaml_field(workflow, "jobs", "workflow")?;
    let jobs = jobs
        .as_hash()
        .ok_or_else(|| LabError("GUI workflow `jobs` must be a mapping".into()))?;
    if jobs.len() != 1 {
        return Err(LabError(format!(
            "GUI workflow must define exactly one job, got {}",
            jobs.len()
        )));
    }
    jobs.get(&yaml_key("gui-smoke"))
        .ok_or_else(|| LabError("GUI workflow must define only the `gui-smoke` job".into()))
}

fn gui_steps(job: &Yaml) -> LabResult<&Vec<Yaml>> {
    yaml_field(job, "steps", "jobs.gui-smoke")?
        .as_vec()
        .ok_or_else(|| LabError("GUI workflow `jobs.gui-smoke.steps` must be a sequence".into()))
}

fn workflow_step(steps: &[Yaml], step: WorkflowStep) -> LabResult<&Yaml> {
    steps.get(step.index()).ok_or_else(|| {
        LabError(format!(
            "GUI workflow is missing expected step {step:?} at index {}",
            step.index()
        ))
    })
}

fn validate_step_shape(steps: &[Yaml]) -> LabResult<()> {
    if steps.len() != WORKFLOW_STEPS.len() {
        return Err(LabError(format!(
            "GUI workflow step cardinality changed: actual={}, expected={}",
            steps.len(),
            WORKFLOW_STEPS.len()
        )));
    }
    for expected in WORKFLOW_STEPS {
        let step = workflow_step(steps, *expected)?;
        let mapping = step
            .as_hash()
            .ok_or_else(|| LabError(format!("GUI workflow step {expected:?} must be a mapping")))?;
        let actual_name = mapping
            .get(&yaml_key("name"))
            .map(|value| yaml_string(value, &format!("steps[{}].name", expected.index())));
        match (expected.name(), actual_name) {
            (None, None) => {}
            (Some(wanted), Some(Ok(actual))) if actual == wanted => {}
            _ => {
                return Err(LabError(format!(
                    "GUI workflow step {:?} name/cardinality changed",
                    expected
                )));
            }
        }
        let has_uses = mapping.contains_key(&yaml_key("uses"));
        let has_run = mapping.contains_key(&yaml_key("run"));
        let action_step = matches!(expected, WorkflowStep::Checkout | WorkflowStep::Upload);
        if action_step != has_uses || action_step == has_run {
            return Err(LabError(format!(
                "GUI workflow step {expected:?} action/run shape changed"
            )));
        }
    }
    Ok(())
}

fn active_run_positions(run: &str, kind: RunMatchKind, text: &str) -> Vec<RunPosition> {
    let mut positions = Vec::new();
    for (line, raw) in run.lines().enumerate() {
        let active = active_shell_code(raw);
        if active.is_empty() {
            continue;
        }
        match kind {
            RunMatchKind::Line if active == text => positions.push(RunPosition { line, column: 0 }),
            RunMatchKind::Line => {}
            RunMatchKind::Token => positions.extend(
                active
                    .match_indices(text)
                    .map(|(column, _)| RunPosition { line, column }),
            ),
        }
    }
    positions
}

fn active_shell_code(line: &str) -> &str {
    analyze_shell_line(line).map_or("", |line| line.code)
}

#[derive(Clone, Copy, Debug)]
struct AnalyzedShellLine<'a> {
    code: &'a str,
    continued: bool,
}

fn shell_comment_boundary(previous: Option<char>) -> bool {
    previous.is_none_or(|character| character.is_whitespace() || ";&|()<> {}".contains(character))
}

fn analyze_shell_line(line: &str) -> LabResult<AnalyzedShellLine<'_>> {
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;
    let mut end = line.len();
    let mut previous = None;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            previous = Some(character);
            continue;
        }
        if character == '\\' && !single_quoted {
            escaped = true;
            previous = Some(character);
            continue;
        }
        if character == '\'' && !double_quoted {
            single_quoted = !single_quoted;
        } else if character == '"' && !single_quoted {
            double_quoted = !double_quoted;
        } else if character == '#'
            && !single_quoted
            && !double_quoted
            && shell_comment_boundary(previous)
        {
            end = index;
            break;
        }
        previous = Some(character);
    }
    if single_quoted || double_quoted {
        return Err(LabError(
            "protected shell program has a quote spanning physical lines".into(),
        ));
    }
    let code = line[..end].trim();
    let continued = escaped && end == line.len();
    let code = if continued {
        code.strip_suffix('\\')
            .ok_or_else(|| {
                LabError("protected shell continuation could not be canonicalized".into())
            })?
            .trim_end()
    } else {
        code
    };
    Ok(AnalyzedShellLine { code, continued })
}

fn canonical_shell_program(run: &str) -> LabResult<Vec<CanonicalShellCommand>> {
    let mut program = Vec::new();
    let mut segments = Vec::new();
    let mut awaiting_continuation = false;
    for raw in run.lines() {
        let line = analyze_shell_line(raw)?;
        if line.code.is_empty() {
            if awaiting_continuation {
                return Err(LabError(
                    "protected shell continuation crosses a blank/comment line".into(),
                ));
            }
            continue;
        }
        segments.push(line.code.to_owned());
        awaiting_continuation = line.continued;
        if !line.continued {
            program.push(CanonicalShellCommand {
                segments: std::mem::take(&mut segments),
            });
        }
    }
    if awaiting_continuation || !segments.is_empty() {
        return Err(LabError(
            "protected shell program ends with an incomplete continuation".into(),
        ));
    }
    Ok(program)
}

fn canonical_program_bytes(program: &[CanonicalShellCommand]) -> Vec<u8> {
    let mut output = Vec::new();
    for command in program {
        output.extend_from_slice(&(command.segments.len() as u64).to_be_bytes());
        for segment in &command.segments {
            output.extend_from_slice(&(segment.len() as u64).to_be_bytes());
            output.extend_from_slice(segment.as_bytes());
        }
    }
    output
}

fn canonical_program_digest(run: &str) -> LabResult<String> {
    let program = canonical_shell_program(run)?;
    Ok(artifact_integrity_for_bytes(&canonical_program_bytes(&program)).sha256)
}

fn validate_protected_programs(steps: &[Yaml]) -> LabResult<()> {
    if PROTECTED_PROGRAM_DIGESTS.len() != 4 {
        return Err(LabError(
            "protected shell program digest cardinality changed".into(),
        ));
    }
    let mut mismatches = Vec::new();
    for (step, expected) in PROTECTED_PROGRAM_DIGESTS {
        let run = yaml_string(
            yaml_field(
                workflow_step(steps, *step)?,
                "run",
                &format!("step.{step:?}"),
            )?,
            &format!("step.{step:?}.run"),
        )?;
        let actual = canonical_program_digest(run)?;
        if actual != *expected {
            mismatches.push(format!(
                "{step:?}:actual_sha256={actual},expected_sha256={expected}"
            ));
        }
    }
    if !mismatches.is_empty() {
        return Err(LabError(format!(
            "GUI workflow protected programs changed: {}",
            mismatches.join("; ")
        )));
    }
    Ok(())
}

fn parsed_semantics_contains(node: &Yaml, needle: &str) -> bool {
    match node {
        Yaml::String(value) => value.contains(needle),
        Yaml::Array(values) => values
            .iter()
            .any(|value| parsed_semantics_contains(value, needle)),
        Yaml::Hash(values) => values.iter().any(|(key, value)| {
            if key.as_str() == Some("run") {
                value.as_str().is_some_and(|run| {
                    run.lines().any(|line| {
                        let active = active_shell_code(line);
                        !active.is_empty() && active.contains(needle)
                    })
                })
            } else {
                parsed_semantics_contains(key, needle) || parsed_semantics_contains(value, needle)
            }
        }),
        _ => false,
    }
}

fn validate_contract_lock_values(lock: &VersionsLock) -> LabResult<()> {
    let expected = BTreeMap::from([
        ("runs-on", lock.ubuntu_gui.runner.as_str()),
        ("RUST_VERSION", lock.toolchain.rust.as_str()),
        ("NODE_VERSION", lock.toolchain.node.as_str()),
        ("NPM_VERSION", lock.toolchain.npm.as_str()),
        ("NODE_URL", lock.toolchain.node_url.as_str()),
        ("NODE_SHA256", lock.toolchain.node_linux_x64_sha256.as_str()),
        ("TAURI_VERSION", lock.toolchain.tauri_cli.as_str()),
        ("TAURI_URL", lock.toolchain.tauri_cli_url.as_str()),
        ("TAURI_SHA512", lock.toolchain.tauri_cli_sha512.as_str()),
    ]);
    for (key, wanted) in expected {
        let entry = WORKFLOW_CONTRACT.iter().find(|entry| match entry.locator {
            WorkflowContractLocator::JobField { key: actual }
            | WorkflowContractLocator::JobEnv { key: actual } => actual == key,
            _ => false,
        });
        if entry.map(|entry| entry.text) != Some(wanted) {
            return Err(LabError(format!(
                "typed workflow contract drifted from lock field `{key}`"
            )));
        }
    }
    Ok(())
}

fn validate_gui_workflow_value(workflow: &Yaml, lock: &VersionsLock) -> LabResult<()> {
    validate_contract_lock_values(lock)?;
    let job = gui_job(workflow)?;
    let steps = gui_steps(job)?;
    validate_step_shape(steps)?;
    validate_protected_programs(steps)?;

    for package in std::iter::once(&lock.ubuntu_gui.kicad_package)
        .chain(lock.ubuntu_gui.stock_packages.iter())
        .chain([
            &lock.ubuntu_gui.webkit_library,
            &lock.ubuntu_gui.webkit_driver,
            &lock.ubuntu_gui.xvfb,
            &lock.ubuntu_gui.x11_utils,
            &lock.ubuntu_gui.imagemagick,
        ])
    {
        if parsed_semantics_contains(workflow, &package.version) {
            return Err(LabError(format!(
                "GUI workflow duplicates locked package version `{}` instead of loading it",
                package.version
            )));
        }
    }
    let mut categories = BTreeMap::new();
    let mut ordering =
        BTreeMap::<(WorkflowStep, WorkflowOrderGroup), Vec<(u16, RunPosition, &'static str)>>::new(
        );
    for required in WORKFLOW_CONTRACT {
        *categories.entry(required.category).or_insert(0) += 1;
        match required.locator {
            WorkflowContractLocator::Uses { step } | WorkflowContractLocator::If { step } => {
                let key = if matches!(required.locator, WorkflowContractLocator::Uses { .. }) {
                    "uses"
                } else {
                    "if"
                };
                let actual = yaml_string(
                    yaml_field(workflow_step(steps, step)?, key, &format!("step.{step:?}"))?,
                    &format!("step.{step:?}.{key}"),
                )?;
                if actual != required.text {
                    return Err(LabError(format!(
                        "GUI workflow field {step:?}.{key} changed"
                    )));
                }
            }
            WorkflowContractLocator::With { step, key } => {
                let with = yaml_field(
                    workflow_step(steps, step)?,
                    "with",
                    &format!("step.{step:?}"),
                )?;
                let actual =
                    yaml_string(yaml_field(with, key, &format!("step.{step:?}.with"))?, key)?;
                if actual != required.text {
                    return Err(LabError(format!(
                        "GUI workflow with field {step:?}.{key} changed"
                    )));
                }
            }
            WorkflowContractLocator::JobField { key } => {
                if yaml_string(yaml_field(job, key, "jobs.gui-smoke")?, key)? != required.text {
                    return Err(LabError(format!("GUI workflow job field `{key}` changed")));
                }
            }
            WorkflowContractLocator::JobEnv { key } => {
                let env = yaml_field(job, "env", "jobs.gui-smoke")?;
                if yaml_string(yaml_field(env, key, "jobs.gui-smoke.env")?, key)? != required.text {
                    return Err(LabError(format!("GUI workflow job env `{key}` changed")));
                }
            }
            WorkflowContractLocator::StepEnv { step, key } => {
                let env = yaml_field(
                    workflow_step(steps, step)?,
                    "env",
                    &format!("step.{step:?}"),
                )?;
                if yaml_string(yaml_field(env, key, &format!("step.{step:?}.env"))?, key)?
                    != required.text
                {
                    return Err(LabError(format!(
                        "GUI workflow step env {step:?}.{key} changed"
                    )));
                }
            }
            WorkflowContractLocator::TriggerPath => {
                let triggers = yaml_field(workflow, "on", "workflow")?;
                let pull_request = yaml_field(triggers, "pull_request", "on")?;
                let paths = yaml_field(pull_request, "paths", "on.pull_request")?
                    .as_vec()
                    .ok_or_else(|| {
                        LabError("GUI workflow pull_request.paths must be a sequence".into())
                    })?;
                let actual = paths
                    .iter()
                    .filter_map(Yaml::as_str)
                    .filter(|path| *path == required.text)
                    .count();
                if actual != 1 {
                    return Err(LabError(format!(
                        "GUI workflow trigger path `{}` must occur exactly once",
                        required.text
                    )));
                }
            }
            WorkflowContractLocator::Run { kind, locations } => {
                for location in locations {
                    let step = workflow_step(steps, location.step)?;
                    let run = yaml_string(
                        yaml_field(step, "run", &format!("step.{:?}", location.step))?,
                        &format!("step.{:?}.run", location.step),
                    )?;
                    let positions = active_run_positions(run, kind, required.text);
                    if positions.len() != location.occurrences {
                        return Err(LabError(format!(
                            "GUI workflow active run contract {:?} in {:?} occurs {} times, expected {}: `{}`",
                            required.category,
                            location.step,
                            positions.len(),
                            location.occurrences,
                            required.text
                        )));
                    }
                    if let Some((group, rank)) = location.order {
                        if positions.len() != 1 {
                            return Err(LabError(format!(
                                "ordered GUI workflow contract must have one occurrence: `{}`",
                                required.text
                            )));
                        }
                        ordering.entry((location.step, group)).or_default().push((
                            rank,
                            positions[0],
                            required.text,
                        ));
                    }
                }
            }
        }
    }
    if categories != expected_workflow_category_counts() {
        return Err(LabError(format!(
            "GUI workflow contract categories shrank or drifted: actual={categories:?}, expected={:?}",
            expected_workflow_category_counts()
        )));
    }
    for ((step, group), entries) in &mut ordering {
        entries.sort_by_key(|(rank, _, _)| *rank);
        for pair in entries.windows(2) {
            let (left_rank, left_position, left_text) = pair[0];
            let (right_rank, right_position, right_text) = pair[1];
            if left_rank >= right_rank || left_position >= right_position {
                return Err(LabError(format!(
                    "GUI workflow order changed in {step:?}/{group:?}: `{left_text}` must precede `{right_text}`"
                )));
            }
        }
    }
    Ok(())
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
            "scope": ["database", "symbols", "footprints", "3d-models", "fixtures/import", "fixtures/kicad-10/afk-smoke"],
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
        fs::create_dir_all(temporary.path().join("fixtures/kicad-10/afk-smoke")).unwrap();
        let plan = RunPlan::create(temporary.path(), Some("missing-artifacts"), &lock).unwrap();
        assert!(write_evidence_bundle(&plan, LaneStatus::Failed, &initial_lanes()).is_err());
    }

    #[test]
    fn checked_in_lock_compose_dockerfiles_and_gui_workflow_match() {
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

    fn yaml_field_mut<'a>(node: &'a mut Yaml, key: &str) -> &'a mut Yaml {
        node.as_mut_hash().unwrap().get_mut(&yaml_key(key)).unwrap()
    }

    fn gui_job_mut(workflow: &mut Yaml) -> &mut Yaml {
        yaml_field_mut(yaml_field_mut(workflow, "jobs"), "gui-smoke")
    }

    fn gui_steps_mut(workflow: &mut Yaml) -> &mut Vec<Yaml> {
        yaml_field_mut(gui_job_mut(workflow), "steps")
            .as_mut_vec()
            .unwrap()
    }

    fn step_field_mut<'a>(workflow: &'a mut Yaml, step: WorkflowStep, key: &str) -> &'a mut Yaml {
        yaml_field_mut(&mut gui_steps_mut(workflow)[step.index()], key)
    }

    fn mapping_change_or_remove(node: &mut Yaml, key: &str, remove: bool) {
        let mapping = node.as_mut_hash().unwrap();
        if remove {
            mapping.remove(&yaml_key(key));
        } else {
            mapping.insert(yaml_key(key), Yaml::String("NEUTRALIZED".into()));
        }
    }

    fn neutralize_field_contract(
        workflow: &mut Yaml,
        locator: WorkflowContractLocator,
        text: &str,
        remove: bool,
    ) {
        match locator {
            WorkflowContractLocator::Uses { step } => {
                mapping_change_or_remove(&mut gui_steps_mut(workflow)[step.index()], "uses", remove)
            }
            WorkflowContractLocator::If { step } => {
                mapping_change_or_remove(&mut gui_steps_mut(workflow)[step.index()], "if", remove)
            }
            WorkflowContractLocator::With { step, key } => {
                mapping_change_or_remove(step_field_mut(workflow, step, "with"), key, remove)
            }
            WorkflowContractLocator::JobField { key } => {
                mapping_change_or_remove(gui_job_mut(workflow), key, remove)
            }
            WorkflowContractLocator::JobEnv { key } => {
                mapping_change_or_remove(yaml_field_mut(gui_job_mut(workflow), "env"), key, remove)
            }
            WorkflowContractLocator::StepEnv { step, key } => {
                mapping_change_or_remove(step_field_mut(workflow, step, "env"), key, remove)
            }
            WorkflowContractLocator::TriggerPath => {
                let triggers = yaml_field_mut(workflow, "on");
                let pull_request = yaml_field_mut(triggers, "pull_request");
                let paths = yaml_field_mut(pull_request, "paths").as_mut_vec().unwrap();
                let index = paths
                    .iter()
                    .position(|value| value.as_str() == Some(text))
                    .unwrap();
                if remove {
                    paths.remove(index);
                } else {
                    paths[index] = Yaml::String("NEUTRALIZED".into());
                }
            }
            WorkflowContractLocator::Run { .. } => unreachable!(),
        }
    }

    fn comment_run_occurrence(
        workflow: &mut Yaml,
        step: WorkflowStep,
        kind: RunMatchKind,
        text: &str,
        occurrence: usize,
    ) {
        let run = step_field_mut(workflow, step, "run");
        let source = run.as_str().unwrap();
        let position = active_run_positions(source, kind, text)[occurrence];
        let mut lines = source.lines().map(str::to_owned).collect::<Vec<_>>();
        lines[position.line] = format!("# {}", lines[position.line]);
        *run = Yaml::String(lines.join("\n") + "\n");
    }

    fn swap_active_lines(workflow: &mut Yaml, step: WorkflowStep, left: &str, right: &str) {
        let run = step_field_mut(workflow, step, "run");
        let source = run.as_str().unwrap();
        let mut lines = source.lines().map(str::to_owned).collect::<Vec<_>>();
        let left = lines.iter().position(|line| line.trim() == left).unwrap();
        let right = lines.iter().position(|line| line.trim() == right).unwrap();
        lines.swap(left, right);
        *run = Yaml::String(lines.join("\n") + "\n");
    }

    fn relocate_install_spec(workflow: &mut Yaml, spec: &str, unpinned: &str, payload: &str) {
        let run = step_field_mut(workflow, WorkflowStep::Install, "run");
        let source = run.as_str().unwrap();
        assert_eq!(source.matches(spec).count(), 1);
        *run = Yaml::String(format!(
            "{}\n{payload}\n",
            source.replacen(spec, unpinned, 1)
        ));
    }

    #[test]
    fn gui_workflow_contract_structurally_rejects_every_locator_mutation() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let lock = VersionsLock::load(
            &repository.join("infra/versions.lock"),
            &repository.join("rust-toolchain.toml"),
        )
        .unwrap();
        let workflow =
            fs::read_to_string(repository.join(".github/workflows/kicad-gui-smoke.yml")).unwrap();
        let parsed = parse_gui_workflow(&workflow).unwrap();
        validate_gui_workflow_value(&parsed, &lock).unwrap();
        let categories = WORKFLOW_CONTRACT.iter().fold(
            BTreeMap::<WorkflowContractCategory, usize>::new(),
            |mut counts, required| {
                *counts.entry(required.category).or_insert(0) += 1;
                counts
            },
        );
        assert_eq!(categories, expected_workflow_category_counts());
        assert_eq!(WORKFLOW_CONTRACT.len(), 95);
        for required in WORKFLOW_CONTRACT {
            match required.locator {
                WorkflowContractLocator::Run { kind, locations } => {
                    for location in locations {
                        for occurrence in 0..location.occurrences {
                            let mut changed = parsed.clone();
                            comment_run_occurrence(
                                &mut changed,
                                location.step,
                                kind,
                                required.text,
                                occurrence,
                            );
                            assert!(
                                validate_gui_workflow_value(&changed, &lock).is_err(),
                                "accepted commented {:?} run contract in {:?}: `{}`",
                                required.category,
                                location.step,
                                required.text
                            );
                        }
                    }
                }
                locator => {
                    for remove in [false, true] {
                        let mut changed = parsed.clone();
                        neutralize_field_contract(&mut changed, locator, required.text, remove);
                        assert!(
                            validate_gui_workflow_value(&changed, &lock).is_err(),
                            "accepted changed/removed {:?} field contract: `{}`",
                            required.category,
                            required.text
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn gui_workflow_scopes_downgrade_permission_to_exact_install() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let lock = VersionsLock::load(
            &repository.join("infra/versions.lock"),
            &repository.join("rust-toolchain.toml"),
        )
        .unwrap();
        let workflow =
            fs::read_to_string(repository.join(".github/workflows/kicad-gui-smoke.yml")).unwrap();
        validate_gui_workflow_contract(&workflow, &lock).unwrap();

        let exact_install = "sudo apt-get install --yes --allow-downgrades \\";
        assert_eq!(workflow.matches(exact_install).count(), 1);
        let omitted = workflow.replacen(exact_install, "sudo apt-get install --yes \\", 1);
        assert!(validate_gui_workflow_contract(&omitted, &lock).is_err());

        let misplaced = omitted.replacen(
            "sudo apt-get update",
            "sudo apt-get update --allow-downgrades",
            1,
        );
        assert!(validate_gui_workflow_contract(&misplaced, &lock).is_err());
    }

    #[test]
    fn gui_workflow_derives_exact_cli_version_from_locked_package() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let lock = VersionsLock::load(
            &repository.join("infra/versions.lock"),
            &repository.join("rust-toolchain.toml"),
        )
        .unwrap();
        let workflow =
            fs::read_to_string(repository.join(".github/workflows/kicad-gui-smoke.yml")).unwrap();

        let exact_check = "grep -Fqx -- \"Version: $kicad_upstream_version-$KICAD_VERSION, release build\" \"$artifact_dir/kicad-version.txt\"";
        let upstream_version = lock
            .ubuntu_gui
            .kicad_package
            .version
            .split_once('~')
            .unwrap()
            .0;
        let upstream_only = format!(
            "grep -Fqx -- \"Version: {upstream_version}, release build\" \"$artifact_dir/kicad-version.txt\""
        );
        let loose_substring = exact_check.replacen("grep -Fqx --", "grep -Fq --", 1);
        let regex_check = exact_check.replacen("grep -Fqx --", "grep -Eqx --", 1);
        for (from, to, case) in [
            (
                "kicad_upstream_version=\"${KICAD_VERSION%%~*}\"",
                ":",
                "removed locked-package derivation",
            ),
            (
                exact_check,
                loose_substring.as_str(),
                "accepted a substring",
            ),
            (
                exact_check,
                regex_check.as_str(),
                "accepted a regular expression",
            ),
            (
                exact_check,
                upstream_only.as_str(),
                "used an upstream-only literal",
            ),
        ] {
            assert_eq!(workflow.matches(from).count(), 1, "fixture drift: {case}");
            let changed = workflow.replacen(from, to, 1);
            assert!(
                validate_gui_workflow_contract(&changed, &lock).is_err(),
                "accepted workflow that {case}"
            );
        }

        let output_line = "          kicad-cli version --format about | tee \"$artifact_dir/kicad-version.txt\"\n";
        let check_line = format!("          {exact_check}\n");
        assert_eq!(workflow.matches(output_line).count(), 1);
        assert_eq!(workflow.matches(&check_line).count(), 1);
        let without_check = workflow.replacen(&check_line, "", 1);
        let misplaced =
            without_check.replacen(output_line, &format!("{check_line}{output_line}"), 1);
        assert!(validate_gui_workflow_contract(&misplaced, &lock).is_err());
    }

    #[test]
    fn gui_workflow_requires_locked_gui_helpers_and_exact_preflights() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let lock = VersionsLock::load(
            &repository.join("infra/versions.lock"),
            &repository.join("rust-toolchain.toml"),
        )
        .unwrap();
        let workflow =
            fs::read_to_string(repository.join(".github/workflows/kicad-gui-smoke.yml")).unwrap();

        assert!(!workflow.contains("x11-apps"));
        assert!(!workflow.contains("xauth"));
        for (spec, floating) in [
            ("\"$X11_UTILS_SPEC\"", "\"$X11_UTILS_PACKAGE\""),
            ("\"$IMAGEMAGICK_SPEC\"", "\"$IMAGEMAGICK_PACKAGE\""),
        ] {
            assert_eq!(workflow.matches(spec).count(), 1);
            for replacement in ["", floating] {
                let changed = workflow.replacen(spec, replacement, 1);
                assert!(
                    validate_gui_workflow_contract(&changed, &lock).is_err(),
                    "accepted removed or floating GUI helper package `{spec}`"
                );
            }
        }

        for (command, path) in [
            ("kicad-cli", "/usr/bin/kicad-cli"),
            ("pcbnew", "/usr/bin/pcbnew"),
            ("Xvfb", "/usr/bin/Xvfb"),
            ("xwininfo", "/usr/bin/xwininfo"),
            ("import", "/usr/bin/import"),
        ] {
            let preflight = format!("test \"$(command -v {command})\" = {path}");
            assert_eq!(workflow.matches(&preflight).count(), 1);
            for replacement in [":".to_owned(), preflight.replace(path, "/usr/bin/false")] {
                let changed = workflow.replacen(&preflight, &replacement, 1);
                assert!(
                    validate_gui_workflow_contract(&changed, &lock).is_err(),
                    "accepted removed or substituted `{command}` preflight"
                );
            }
        }

        let manifest_tail = " \"$XVFB_PACKAGE\" \"$X11_UTILS_PACKAGE\" \"$IMAGEMAGICK_PACKAGE\"";
        assert_eq!(workflow.matches(manifest_tail).count(), 1);
        let incomplete_manifest = workflow.replacen(manifest_tail, " \"$XVFB_PACKAGE\"", 1);
        assert!(validate_gui_workflow_contract(&incomplete_manifest, &lock).is_err());
    }

    #[test]
    fn gui_workflow_requires_private_pinned_node_and_npm() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let lock = VersionsLock::load(
            &repository.join("infra/versions.lock"),
            &repository.join("rust-toolchain.toml"),
        )
        .unwrap();
        let workflow =
            fs::read_to_string(repository.join(".github/workflows/kicad-gui-smoke.yml")).unwrap();

        for (from, to, case) in [
            (
                "tar -xJf /tmp/node.tar.xz --strip-components=1 -C \"$toolchain_prefix\"",
                "sudo tar -xJf /tmp/node.tar.xz --strip-components=1 -C /usr/local",
                "global sudo extraction",
            ),
            (
                "test ! -e \"$toolchain_prefix\"",
                ":",
                "removed fresh-prefix assertion",
            ),
            (
                "mkdir \"$toolchain_prefix\"",
                "mkdir -p \"$toolchain_prefix\"",
                "allowed a preexisting prefix",
            ),
            (
                "export PATH=\"$toolchain_prefix/bin:$PATH\"",
                ":",
                "removed current-step private PATH",
            ),
            (
                "\"$toolchain_prefix/bin/npm\" install",
                "npm install",
                "selected implicit npm",
            ),
            (
                "$(\"$toolchain_prefix/bin/node\" --version)",
                "$(node --version)",
                "selected implicit node probe",
            ),
            (
                "$(\"$toolchain_prefix/bin/npm\" --version)",
                "$(npm --version)",
                "selected implicit npm probe",
            ),
            (
                "echo \"$toolchain_prefix/bin\" >> \"$GITHUB_PATH\"",
                "echo \"/usr/local/bin\" >> \"$GITHUB_PATH\"",
                "propagated the global tool path",
            ),
        ] {
            assert_eq!(workflow.matches(from).count(), 1, "fixture drift: {case}");
            let changed = workflow.replacen(from, to, 1);
            assert!(
                validate_gui_workflow_contract(&changed, &lock).is_err(),
                "accepted workflow with {case}"
            );
        }

        let path_line = "          echo \"$toolchain_prefix/bin\" >> \"$GITHUB_PATH\"\n";
        let probe_line = "          test \"$(\"$toolchain_prefix/bin/tauri\" --version)\" = \"tauri-cli $TAURI_VERSION\"\n";
        assert_eq!(workflow.matches(path_line).count(), 1);
        assert_eq!(workflow.matches(probe_line).count(), 1);
        let without_path = workflow.replacen(path_line, "", 1);
        let misplaced = without_path.replacen(probe_line, &format!("{probe_line}{path_line}"), 1);
        assert!(validate_gui_workflow_contract(&misplaced, &lock).is_err());
    }

    #[test]
    fn gui_workflow_requires_verified_offline_private_tauri_install_and_logs() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let lock = VersionsLock::load(
            &repository.join("infra/versions.lock"),
            &repository.join("rust-toolchain.toml"),
        )
        .unwrap();
        let workflow =
            fs::read_to_string(repository.join(".github/workflows/kicad-gui-smoke.yml")).unwrap();

        for (from, to, case) in [
            (
                "echo \"$TAURI_LINUX_X64_GNU_SHA512  /tmp/tauri-cli-linux-x64-gnu.tgz\" | sha512sum -c -",
                ":",
                "removed native checksum",
            ),
            (
                "$TAURI_LINUX_X64_GNU_SHA512  /tmp/tauri-cli-linux-x64-gnu.tgz",
                "$TAURI_SHA512  /tmp/tauri-cli-linux-x64-gnu.tgz",
                "substituted wrapper checksum",
            ),
            (" --offline ", " ", "removed offline mode"),
            (
                "--global --prefix \"$toolchain_prefix\"",
                "--global --prefix /usr/local",
                "replaced private prefix",
            ),
            (
                "/tmp/tauri-cli.tgz /tmp/tauri-cli-linux-x64-gnu.tgz",
                "/tmp/tauri-cli.tgz",
                "removed native tarball operand",
            ),
            (
                "--logs-dir \"$npm_logs\"",
                "--logs-dir /tmp/npm-logs",
                "moved debug logs outside evidence",
            ),
            (
                "2>&1 | tee \"$artifact_dir/npm-tauri-install.log\"",
                "2>&1",
                "removed persistent console log",
            ),
        ] {
            assert_eq!(workflow.matches(from).count(), 1, "fixture drift: {case}");
            let changed = workflow.replacen(from, to, 1);
            assert!(
                validate_gui_workflow_contract(&changed, &lock).is_err(),
                "accepted workflow with {case}"
            );
        }

        let checksum_line = "          echo \"$TAURI_LINUX_X64_GNU_SHA512  /tmp/tauri-cli-linux-x64-gnu.tgz\" | sha512sum -c -\n";
        let path_line = "          echo \"$toolchain_prefix/bin\" >> \"$GITHUB_PATH\"\n";
        assert_eq!(workflow.matches(checksum_line).count(), 1);
        assert_eq!(workflow.matches(path_line).count(), 1);
        let without_checksum = workflow.replacen(checksum_line, "", 1);
        let misplaced =
            without_checksum.replacen(path_line, &format!("{path_line}{checksum_line}"), 1);
        assert!(validate_gui_workflow_contract(&misplaced, &lock).is_err());
    }

    #[test]
    fn gui_workflow_comments_dead_metadata_parse_and_order_fail_closed() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let lock = VersionsLock::load(
            &repository.join("infra/versions.lock"),
            &repository.join("rust-toolchain.toml"),
        )
        .unwrap();
        let workflow =
            fs::read_to_string(repository.join(".github/workflows/kicad-gui-smoke.yml")).unwrap();
        let exporter = "cargo xtask export-github-env >> \"$GITHUB_ENV\"";
        let commented = workflow.replacen(exporter, &format!("# {exporter}"), 1);
        assert!(validate_gui_workflow_contract(&commented, &lock).is_err());
        assert!(validate_gui_workflow_contract("jobs: [", &lock).is_err());

        let mut dead_metadata = parse_gui_workflow(&workflow).unwrap();
        comment_run_occurrence(
            &mut dead_metadata,
            WorkflowStep::Export,
            RunMatchKind::Line,
            exporter,
            0,
        );
        gui_steps_mut(&mut dead_metadata)[WorkflowStep::EnsureEvidence.index()]
            .as_mut_hash()
            .unwrap()
            .insert(yaml_key("x-dead-contract"), Yaml::String(exporter.into()));
        assert!(validate_gui_workflow_value(&dead_metadata, &lock).is_err());

        let mut unrelated_run = parse_gui_workflow(&workflow).unwrap();
        comment_run_occurrence(
            &mut unrelated_run,
            WorkflowStep::Export,
            RunMatchKind::Line,
            exporter,
            0,
        );
        let ensure_run = step_field_mut(&mut unrelated_run, WorkflowStep::EnsureEvidence, "run");
        *ensure_run = Yaml::String(format!("{}\n{exporter}\n", ensure_run.as_str().unwrap()));
        assert!(validate_gui_workflow_value(&unrelated_run, &lock).is_err());

        let mut token_in_name = parse_gui_workflow(&workflow).unwrap();
        comment_run_occurrence(
            &mut token_in_name,
            WorkflowStep::Export,
            RunMatchKind::Line,
            exporter,
            0,
        );
        *step_field_mut(&mut token_in_name, WorkflowStep::EnsureEvidence, "name") =
            Yaml::String(exporter.into());
        assert!(validate_gui_workflow_value(&token_in_name, &lock).is_err());

        let mut unexpected = parse_gui_workflow(&workflow).unwrap();
        let duplicate = gui_steps_mut(&mut unexpected)[WorkflowStep::Export.index()].clone();
        gui_steps_mut(&mut unexpected).push(duplicate);
        assert!(validate_gui_workflow_value(&unexpected, &lock).is_err());

        let mut reordered = parse_gui_workflow(&workflow).unwrap();
        swap_active_lines(
            &mut reordered,
            WorkflowStep::Install,
            "sudo add-apt-repository --yes \"$KICAD_PPA\"",
            "apt-cache madison \"$KICAD_PACKAGE\" | awk '{print $3}' | grep -Fqx -- \"$KICAD_VERSION\"",
        );
        assert!(validate_gui_workflow_value(&reordered, &lock).is_err());

        let harmless_comment = format!(
            "{workflow}\n# {} is documentation, not executable metadata\n",
            lock.ubuntu_gui.webkit_driver.version
        );
        validate_gui_workflow_contract(&harmless_comment, &lock).unwrap();

        let mut harmless_shell_comment = parse_gui_workflow(&workflow).unwrap();
        let install_run = step_field_mut(&mut harmless_shell_comment, WorkflowStep::Install, "run");
        *install_run = Yaml::String(format!(
            "{}\n# {} is not an active package pin\n",
            install_run.as_str().unwrap(),
            lock.ubuntu_gui.webkit_driver.version
        ));
        validate_gui_workflow_value(&harmless_shell_comment, &lock).unwrap();

        let mut inline_comment = parse_gui_workflow(&workflow).unwrap();
        let install_run = step_field_mut(&mut inline_comment, WorkflowStep::Install, "run");
        *install_run = Yaml::String(format!(
            "{}\n: # \"$KICAD_SPEC\" is not executable\n",
            install_run
                .as_str()
                .unwrap()
                .replacen("\"$KICAD_SPEC\"", "", 1)
        ));
        assert!(validate_gui_workflow_value(&inline_comment, &lock).is_err());
    }

    #[test]
    fn protected_program_digest_rejects_data_dead_code_and_continuation_bypasses() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let lock = VersionsLock::load(
            &repository.join("infra/versions.lock"),
            &repository.join("rust-toolchain.toml"),
        )
        .unwrap();
        let workflow =
            fs::read_to_string(repository.join(".github/workflows/kicad-gui-smoke.yml")).unwrap();
        let parsed = parse_gui_workflow(&workflow).unwrap();
        validate_gui_workflow_value(&parsed, &lock).unwrap();

        let install_specs = [
            ("\"$KICAD_SPEC\"", "\"$KICAD_PACKAGE\""),
            ("\"$KICAD_SYMBOLS_SPEC\"", "\"$KICAD_SYMBOLS_PACKAGE\""),
            (
                "\"$KICAD_FOOTPRINTS_SPEC\"",
                "\"$KICAD_FOOTPRINTS_PACKAGE\"",
            ),
            (
                "\"$KICAD_PACKAGES3D_SPEC\"",
                "\"$KICAD_PACKAGES3D_PACKAGE\"",
            ),
            ("\"$WEBKIT_LIBRARY_SPEC\"", "\"$WEBKIT_LIBRARY_PACKAGE\""),
            ("\"$WEBKIT_DRIVER_SPEC\"", "\"$WEBKIT_DRIVER_PACKAGE\""),
            ("\"$XVFB_SPEC\"", "\"$XVFB_PACKAGE\""),
            ("\"$X11_UTILS_SPEC\"", "\"$X11_UTILS_PACKAGE\""),
            ("\"$IMAGEMAGICK_SPEC\"", "\"$IMAGEMAGICK_PACKAGE\""),
        ];
        for (spec, unpinned) in install_specs {
            for payload in [
                format!("printf '%s\\n' '{spec}'"),
                format!("echo '{spec}'"),
                format!(":;# {spec}"),
                format!("if false; then\n  printf '%s\\n' '{spec}'\nfi"),
                format!("relocated_spec() {{\n  printf '%s\\n' '{spec}'\n}}"),
                format!("relocated=$(printf '%s' '{spec}')"),
                format!("relocated_data='{spec}'"),
            ] {
                let mut changed = parsed.clone();
                relocate_install_spec(&mut changed, spec, unpinned, &payload);
                assert!(
                    validate_gui_workflow_value(&changed, &lock).is_err(),
                    "accepted protected value relocation for {spec}: {payload}"
                );
            }

            let mut old_matcher_bypass = parsed.clone();
            let payload = format!("printf '%s\\n' '{spec}'");
            relocate_install_spec(&mut old_matcher_bypass, spec, unpinned, &payload);
            let run = step_field_mut(&mut old_matcher_bypass, WorkflowStep::Install, "run")
                .as_str()
                .unwrap();
            assert_eq!(
                active_run_positions(run, RunMatchKind::Token, spec).len(),
                1,
                "reviewer relocation must still satisfy the old substring matcher"
            );
        }

        let mut reshaped = parsed.clone();
        let run = step_field_mut(&mut reshaped, WorkflowStep::Install, "run");
        let source = run.as_str().unwrap();
        let old = "\"$KICAD_SPEC\" \"$KICAD_SYMBOLS_SPEC\" \"$KICAD_FOOTPRINTS_SPEC\" \\";
        let new =
            "\"$KICAD_SPEC\" \\\n            \"$KICAD_SYMBOLS_SPEC\" \"$KICAD_FOOTPRINTS_SPEC\" \\";
        assert!(source.contains(old));
        *run = Yaml::String(source.replacen(old, new, 1));
        assert!(validate_gui_workflow_value(&reshaped, &lock).is_err());

        for step in [
            WorkflowStep::Export,
            WorkflowStep::Install,
            WorkflowStep::Smoke,
            WorkflowStep::EnsureEvidence,
        ] {
            let mut unexpected = parsed.clone();
            let run = step_field_mut(&mut unexpected, step, "run");
            *run = Yaml::String(format!("{}\ntrue\n", run.as_str().unwrap()));
            assert!(
                validate_gui_workflow_value(&unexpected, &lock).is_err(),
                "accepted unexpected executable command in {step:?}"
            );

            let mut harmless = parsed.clone();
            let run = step_field_mut(&mut harmless, step, "run");
            *run = Yaml::String(format!(
                "\n# harmless full-line comment\n{}\n# trailing comment\n",
                run.as_str().unwrap()
            ));
            validate_gui_workflow_value(&harmless, &lock).unwrap();
        }
    }

    #[test]
    fn protected_program_lexer_handles_boundaries_and_fails_ambiguous_input() {
        assert_eq!(active_shell_code(":;# hidden"), ":;");
        assert_eq!(active_shell_code("echo '# data'"), "echo '# data'");
        assert_eq!(active_shell_code("echo foo#bar"), "echo foo#bar");
        assert_eq!(active_shell_code("echo foo |# hidden"), "echo foo |");
        assert!(canonical_shell_program("echo 'unterminated\n").is_err());
        assert!(
            canonical_shell_program(
                "echo value \\\n# comment interrupts continuation\necho other\n"
            )
            .is_err()
        );
    }

    #[test]
    fn github_environment_export_is_typed_deterministic_and_complete() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let lock = VersionsLock::load(
            &repository.join("infra/versions.lock"),
            &repository.join("rust-toolchain.toml"),
        )
        .unwrap();
        let first = github_env_export(&lock).unwrap();
        let second = github_env_export(&lock).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.lines().count(), 33);
        assert!(first.lines().any(|line| {
            line == format!(
                "TAURI_LINUX_X64_GNU_URL={}",
                lock.toolchain.tauri_cli_linux_x64_gnu_url
            )
        }));
        assert!(first.lines().any(|line| {
            line == format!(
                "TAURI_LINUX_X64_GNU_SHA512={}",
                lock.toolchain.tauri_cli_linux_x64_gnu_sha512
            )
        }));
        for prefix in [
            "KICAD",
            "KICAD_SYMBOLS",
            "KICAD_FOOTPRINTS",
            "KICAD_PACKAGES3D",
            "WEBKIT_LIBRARY",
            "WEBKIT_DRIVER",
            "XVFB",
            "X11_UTILS",
            "IMAGEMAGICK",
        ] {
            assert!(
                first
                    .lines()
                    .any(|line| line.starts_with(&format!("{prefix}_PACKAGE=")))
            );
            assert!(
                first
                    .lines()
                    .any(|line| line.starts_with(&format!("{prefix}_VERSION=")))
            );
            assert!(
                first
                    .lines()
                    .any(|line| line.starts_with(&format!("{prefix}_SPEC=")))
            );
        }
        assert!(parse_cli(vec!["export-github-env".into()]).is_ok());
        assert!(parse_cli(vec!["export-github-env".into(), "extra".into()]).is_err());
    }

    #[test]
    fn owned_runtime_scope_rejects_forbidden_scripting_dependencies() {
        let temporary = tempdir().unwrap();
        let safe = temporary.path().join("safe.txt");
        fs::write(&safe, "cargo xtask export-github-env\n").unwrap();
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
