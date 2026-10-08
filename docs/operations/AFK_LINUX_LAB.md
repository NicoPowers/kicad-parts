# AFK Docker application lab

The AFK lab is the reproducible Docker environment for application development and tests. It is deferred while development runs natively on Windows; see `kicad-parts-c748` before relying on it. Its single entry point is:

```text
cargo xtask test-afk
```

The command validates `infra/versions.lock`, reserves a unique Compose project, renders and checks the normalized Compose model, builds the pinned images, starts PocketBase and MinIO, inventories the Rust/Node/npm/Tauri/WebKitGTK test runner, verifies internal connectivity and runtime isolation, writes evidence, and removes its scoped resources. Use `cargo xtask test-afk --static-only` for lock, Dockerfile, and normalized-policy checks without building or starting services.

## Scope

The stack contains exactly three services:

- PocketBase, with a project-scoped named volume and no published host port.
- MinIO, built from its verified source lock, with run-scoped in-memory credentials and a project-scoped named volume.
- A non-root test runner containing the pinned Rust, Node, npm, Tauri, WebKitGTK, and browser-test dependencies.

The images do not include KiCad yet. KiCad 10 is a required product dependency, and tests may run `kicad-cli`. To add it to the lab, pin it in `infra/versions.lock` and add its download or package to the test-runner contract in `tooling/afk/src/dockerfile.rs`. Each image may contain only what its contract lists.

## Isolation and resource policy

No AFK service receives a host bind mount. The repository, host home, native application installation or configuration, Docker socket, SSH agent, credential stores, `secrets.env`, and existing service state are not mounted or sent to containers. Build contexts are limited to the three directories under `infra/docker`. The network is internal and has no published host ports.

Every service uses a read-only root filesystem and UID/GID 65532. PocketBase is limited to 256 MiB and 128 PIDs; MinIO to 1 GiB and 256 PIDs; the test runner to 2 GiB and 512 PIDs. Each service receives exact bounded tmpfs mounts for `/tmp`, `/afk/home`, and `/afk/xdg`. The normalized-policy validator rejects missing, changed, oversized, duplicate, additional, bind, or privileged mounts and settings before a build starts. Runtime probes verify cgroup ceilings, tmpfs owner/mode/size/options, writable XDG leaves, and create-read-delete canaries.

## Evidence and cleanup

Evidence is written to `.afk/runs/<run-id>/artifacts/`. It includes normalized Compose policy, build/start/version/connectivity logs, runtime isolation, environment and image content IDs, teardown status, JUnit XML, and a final evidence manifest. Each declared artifact is a regular non-link file with byte length and SHA-256; undeclared files, directories, links/reparse points, devices, sockets, or metadata errors fail closed. Credentials remain in process memory and child environment only, and secret values are redacted from logs.

Normal success and failure tear down only the run's containers, network, named volumes, and image tags, then verify that no scoped resources remain. `--preserve-on-failure` retains that single run for diagnosis and prints `cargo xtask cleanup-afk --run-id <run-id>`. Reusing a run ID is rejected instead of overwriting evidence.

## Pinned-source notes

MinIO's selected release is built from the exact verified upstream commit because no matching official runtime image exists. Docker helper images are immutable digest references. The Rust base uses the Amazon Public ECR mirror of Docker Official Images, and the Go compiler, Node distribution, Tauri package, PocketBase asset, and MinIO source archive are checksum-verified from `infra/versions.lock`.
