//! What went wrong with a library repo, in plain words: which step failed,
//! why, and what to check, in one line. git's own words go in the Details.
//!
//! git and ssh say what went wrong in English (the app runs them with
//! `LC_ALL=C`), in words that have stayed the same for years. Each rule
//! below matches words they print; the tests hold real messages from git,
//! OpenSSH, GitHub, GitLab and Bitbucket.

use super::address::RepoAddress;
use crate::git_program::Ran;
use desktop_contract::RepoProblem;
use std::time::Duration;

/// The step with a library repo that failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Step {
    /// Reading the repo (`git ls-remote`).
    Read,
    /// Making the app's own copy (`git clone`).
    Copy,
    /// Bringing the app's copy up to date (`git fetch`).
    Fetch,
    /// Trying a push (`git push --dry-run`), after reading worked.
    Write,
}

impl Step {
    /// How the one line starts.
    fn opening(self) -> &'static str {
        match self {
            Step::Read => "Can't read the repo",
            Step::Copy => "Can't copy the repo to this computer",
            Step::Fetch => "Can't fetch the repo",
            Step::Write => "Can read, but can't push",
        }
    }
}

/// Why `step` failed, from what git said in `ran`, for the repo at
/// `address`. `limit` is the step's time limit, said if git ran out of
/// time. `may_prompt` says git or ssh could have been waiting on a hidden
/// question: the person has an SSH command of their own, or the address is
/// https, where a credential helper may ask.
pub(crate) fn explain(
    step: Step,
    ran: &Ran,
    address: &RepoAddress,
    limit: Duration,
    may_prompt: bool,
) -> RepoProblem {
    let host = address.host();
    let host = host.as_str();
    let said = ran.stderr.trim();
    let mut details = if said.is_empty() {
        "git gave no reason.".to_owned()
    } else {
        said.to_owned()
    };
    if ran.timed_out {
        details.push_str(&format!(
            "\nThe app stopped git after {} seconds.",
            limit.as_secs()
        ));
    }
    let (reason, next, offline) = if ran.timed_out && may_prompt {
        (
            format!("{host} did not answer within {} seconds", limit.as_secs()),
            format!(
                "git or ssh may be waiting for a passphrase or sign-in the app can't show. Try git ls-remote {} in a terminal.",
                address.url()
            ),
            false,
        )
    } else if ran.timed_out {
        (
            format!("{host} did not answer within {} seconds", limit.as_secs()),
            "Check your internet connection, or try again later.".to_owned(),
            true,
        )
    } else {
        reason_for(step, &said.to_lowercase(), host, address.is_folder())
    };
    RepoProblem {
        summary: format!("{}: {reason}. {next}", step.opening()),
        details,
        offline,
    }
}

/// What to say when git is not installed: the step, and where to get git
/// on this computer's system.
pub(crate) fn git_not_installed(step: Step) -> RepoProblem {
    let next = if cfg!(windows) {
        "Install Git for Windows from git-scm.com, then check again."
    } else if cfg!(target_os = "macos") {
        "Install git by running xcode-select --install in Terminal, or from git-scm.com, then check again."
    } else {
        "Install git with your system's package manager, such as sudo apt install git, then check again."
    };
    RepoProblem {
        summary: format!(
            "{}: git is not installed on this computer. {next}",
            step.opening()
        ),
        details: "The app could not start the git program. It uses your own git, so your SSH keys and sign-ins work as they do in a terminal.".to_owned(),
        offline: false,
    }
}

/// The reason, the next step, and whether the host could not be reached,
/// for git's message `said` (in small letters).
fn reason_for(
    step: Step,
    said: &str,
    host: &str,
    on_this_computer: bool,
) -> (String, String, bool) {
    let has = |words: &[&str]| words.iter().any(|word| said.contains(word));
    let plain = |reason: String, next: String| (reason, next, false);
    let offline = |reason: String, next: &str| (reason, next.to_owned(), true);
    let connection_hint = "Check your internet connection, or try again later.";
    if has(&["remote host identification has changed"]) {
        return plain(
            format!("{host}'s SSH host key has changed"),
            format!(
                "Ask whoever runs {host} whether its key changed. Then remove the old key with ssh-keygen -R {host}."
            ),
        );
    }
    if has(&[
        "host key verification failed",
        "host key is known for",
        "authenticity of host",
    ]) {
        return plain(
            format!("this computer does not know {host}'s SSH host key yet"),
            format!(
                "Connect once in a terminal, such as with ssh -T git@{host}, and check and accept its key."
            ),
        );
    }
    if has(&[
        "permission denied (publickey",
        "permission denied (keyboard-interactive",
    ]) {
        return plain(
            format!("{host} did not accept your SSH key"),
            format!(
                "Check that your SSH key is added to your account on {host} and loaded in your SSH agent."
            ),
        );
    }
    if has(&[
        "terminal prompts disabled",
        "could not read username",
        "could not read password",
    ]) {
        return plain(
            format!("git has no sign-in for {host}"),
            format!(
                "Sign in to {host} with a git credential helper, such as Git Credential Manager, or use the repo's ssh address."
            ),
        );
    }
    if has(&["authentication failed", "invalid username or password"]) {
        return plain(
            format!("{host} did not accept your sign-in"),
            "Sign in again with your git credential helper, or use the repo's ssh address."
                .to_owned(),
        );
    }
    if has(&[
        "ssl certificate problem",
        "certificate verify failed",
        "server certificate verification failed",
    ]) {
        return plain(
            format!("git can't check {host}'s certificate"),
            format!("Ask whoever runs {host} which certificate git should trust."),
        );
    }
    if has(&[
        "could not resolve hostname",
        "could not resolve host",
        "name or service not known",
        "no such host is known",
        "nodename nor servname",
        "temporary failure in name resolution",
    ]) {
        return offline(
            format!("can't find {host}"),
            "Check the address and your internet connection.",
        );
    }
    if has(&["timed out", "timeout"]) {
        return offline(format!("{host} did not answer"), connection_hint);
    }
    if has(&[
        "connection refused",
        "network is unreachable",
        "no route to host",
        "connect to server",
        "failed to connect",
        "unable to connect",
        "connection reset",
        "connection closed by",
        "kex_exchange_identification",
    ]) {
        return offline(format!("can't reach {host}"), connection_hint);
    }
    if has(&[
        "repository not found",
        "does not appear to be a git repository",
        "could not be found",
        "does not exist",
        "no such repository",
        "returned error: 404",
    ]) || (said.contains("repository '") && said.contains("' not found"))
    {
        let next = if on_this_computer {
            "Check that its folder is there and holds the repo.".to_owned()
        } else {
            format!("Check the address, and that your account on {host} can see the repo.")
        };
        return plain("the repo was not found".to_owned(), next);
    }
    if has(&[
        "permission to",
        "not allowed to push",
        "you are not allowed",
        "write access",
        "access denied",
        "read-only",
        "returned error: 403",
        "forbidden",
        "permission denied",
    ]) {
        let next = match (step, on_this_computer) {
            (Step::Write, false) => {
                format!("Check that your SSH key or account has write access on {host}.")
            }
            (Step::Write, true) => {
                "Check that you may change files in the repo's folder.".to_owned()
            }
            (_, false) => format!("Check that your account on {host} can see the repo."),
            (_, true) => "Check that you may read the repo's folder.".to_owned(),
        };
        return plain("permission denied".to_owned(), next);
    }
    if has(&["dubious ownership"]) {
        return plain(
            "git does not trust the folder's owner".to_owned(),
            "Run the git config command git suggests in Details, then try again.".to_owned(),
        );
    }
    let first = said
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it gave no reason");
    let first = first
        .trim_start_matches("fatal: ")
        .trim_start_matches("error: ")
        .trim_end_matches('.');
    plain(
        format!("git said \"{first}\""),
        "See Details for git's whole message.".to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: Duration = Duration::from_secs(60);
    /// What git adds after ssh's own message.
    const COULD_NOT_READ: &str = "fatal: Could not read from remote repository.\n\nPlease make sure you have the correct access rights\nand the repository exists.";

    fn gitlab() -> RepoAddress {
        RepoAddress::parse("git@gitlab.com:acme/lib.git").unwrap()
    }

    #[test]
    fn a_time_out_that_may_be_a_hidden_question_says_so() {
        let ran = Ran {
            timed_out: true,
            ..Ran::default()
        };
        let web = RepoAddress::parse("https://gitlab.com/acme/lib.git").unwrap();
        let problem = explain(Step::Read, &ran, &web, LIMIT, true);
        assert_eq!(
            problem.summary,
            "Can't read the repo: gitlab.com did not answer within 60 seconds. git or ssh may be waiting for a passphrase or sign-in the app can't show. Try git ls-remote https://gitlab.com/acme/lib.git in a terminal."
        );
        assert!(!problem.offline);
    }

    fn said(step: Step, stderr: &str) -> String {
        let ran = Ran {
            stderr: stderr.to_owned(),
            ..Ran::default()
        };
        explain(step, &ran, &gitlab(), LIMIT, false).summary
    }

    #[test]
    fn ssh_s_own_problems_say_what_to_check() {
        // OpenSSH with BatchMode=yes, meeting a host for the first time.
        assert_eq!(
            said(
                Step::Read,
                &format!("Host key verification failed.\n{COULD_NOT_READ}")
            ),
            "Can't read the repo: this computer does not know gitlab.com's SSH host key yet. Connect once in a terminal, such as with ssh -T git@gitlab.com, and check and accept its key."
        );
        assert!(said(
            Step::Read,
            "No ED25519 host key is known for gitlab.com and you have requested strict checking.\nHost key verification failed."
        )
        .contains("does not know gitlab.com's SSH host key"));
        assert!(said(
            Step::Read,
            "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\nHost key verification failed."
        )
        .contains("SSH host key has changed"));
        // No key the host accepts, or a key with a passphrase and no agent
        // (BatchMode never asks for it).
        assert_eq!(
            said(
                Step::Read,
                &format!(
                    "Warning: Permanently added 'gitlab.com' (ED25519) to the list of known hosts.\ngit@gitlab.com: Permission denied (publickey).\n{COULD_NOT_READ}"
                )
            ),
            "Can't read the repo: gitlab.com did not accept your SSH key. Check that your SSH key is added to your account on gitlab.com and loaded in your SSH agent."
        );
    }

    #[test]
    fn a_host_that_cannot_be_reached_is_said_as_offline() {
        let offline = |stderr: &str| {
            let ran = Ran {
                stderr: stderr.to_owned(),
                ..Ran::default()
            };
            explain(Step::Fetch, &ran, &gitlab(), LIMIT, false)
        };
        // Real messages from Git for Windows 2.53 and its OpenSSH.
        for (stderr, reason) in [
            (
                "ssh: Could not resolve hostname gitlab.com: Name or service not known",
                "can't find gitlab.com",
            ),
            (
                "ssh: Could not resolve hostname gitlab.com: No such host is known. ",
                "can't find gitlab.com",
            ),
            (
                "fatal: unable to access 'https://gitlab.com/acme/lib.git/': Could not resolve host: gitlab.com",
                "can't find gitlab.com",
            ),
            (
                "ssh: connect to host gitlab.com port 22: Connection refused",
                "can't reach gitlab.com",
            ),
            (
                "fatal: unable to access 'https://gitlab.com/acme/lib.git/': Failed to connect to gitlab.com port 443 after 2065 ms: Could not connect to server",
                "can't reach gitlab.com",
            ),
            (
                "ssh: connect to host gitlab.com port 22: Network is unreachable",
                "can't reach gitlab.com",
            ),
            (
                "fatal: unable to connect to gitlab.com:\ngitlab.com[0: 1.2.3.4]: errno=Connection refused",
                "can't reach gitlab.com",
            ),
            (
                "ssh: connect to host gitlab.com port 22: Connection timed out",
                "gitlab.com did not answer",
            ),
            (
                "kex_exchange_identification: Connection closed by remote host\nConnection closed by 1.2.3.4 port 22",
                "can't reach gitlab.com",
            ),
        ] {
            let problem = offline(stderr);
            assert!(problem.offline, "{stderr}");
            assert!(
                problem
                    .summary
                    .starts_with(&format!("Can't fetch the repo: {reason}. ")),
                "{stderr}: {}",
                problem.summary
            );
            assert_eq!(problem.details, stderr.trim());
        }
        // A step the app stopped at its time limit says so in the details.
        let ran = Ran {
            timed_out: true,
            ..Ran::default()
        };
        let problem = explain(Step::Read, &ran, &gitlab(), LIMIT, false);
        assert!(problem.offline);
        assert_eq!(
            problem.summary,
            "Can't read the repo: gitlab.com did not answer within 60 seconds. Check your internet connection, or try again later."
        );
        assert_eq!(
            problem.details,
            "git gave no reason.\nThe app stopped git after 60 seconds."
        );
    }

    #[test]
    fn a_repo_that_is_not_there_is_said_as_not_found() {
        for stderr in [
            // GitHub over ssh and https.
            "ERROR: Repository not found.\nfatal: Could not read from remote repository.",
            "remote: Repository not found.\nfatal: repository 'https://github.com/acme/lib.git/' not found",
            // GitLab.
            "remote: \nremote: ========================================================================\nremote: \nremote: The project you were looking for could not be found or you don't have permission to view it.",
            // Bitbucket.
            "conq: repository does not exist.\nfatal: Could not read from remote repository.",
            // A folder that is no repo.
            "fatal: 'C:/repos/nothing.git' does not appear to be a git repository\nfatal: Could not read from remote repository.",
        ] {
            assert_eq!(
                said(Step::Read, stderr),
                "Can't read the repo: the repo was not found. Check the address, and that your account on gitlab.com can see the repo.",
                "{stderr}"
            );
        }
    }

    #[test]
    fn a_repo_in_a_folder_is_checked_as_a_folder() {
        let ran = Ran {
            stderr: "fatal: '//nas/hw/lib.git' does not appear to be a git repository".to_owned(),
            ..Ran::default()
        };
        assert_eq!(
            explain(
                Step::Read,
                &ran,
                &RepoAddress::parse("/srv/hw/lib.git").unwrap(),
                LIMIT,
                false
            )
            .summary,
            "Can't read the repo: the repo was not found. Check that its folder is there and holds the repo."
        );
    }

    #[test]
    fn a_push_the_host_refuses_says_to_check_write_access() {
        for stderr in [
            // GitHub, a read-only deploy key or a collaborator who may only read.
            "ERROR: Permission to acme/lib.git denied to deploy key\nfatal: Could not read from remote repository.",
            "remote: Permission to acme/lib.git denied to ada.\nfatal: unable to access 'https://github.com/acme/lib.git/': The requested URL returned error: 403",
            // GitLab, a Reporter.
            "remote: \nremote: ========================================================================\nremote: \nremote: You are not allowed to push code to this project.",
            // Bitbucket, a read-only access key.
            "repository access denied. access via a deployment key is read-only.\nfatal: Could not read from remote repository.",
        ] {
            assert_eq!(
                said(Step::Write, stderr),
                "Can read, but can't push: permission denied. Check that your SSH key or account has write access on gitlab.com.",
                "{stderr}"
            );
        }
    }

    #[test]
    fn https_sign_in_problems_name_the_credential_helper() {
        assert_eq!(
            said(
                Step::Read,
                "fatal: could not read Username for 'https://gitlab.com': terminal prompts disabled"
            ),
            "Can't read the repo: git has no sign-in for gitlab.com. Sign in to gitlab.com with a git credential helper, such as Git Credential Manager, or use the repo's ssh address."
        );
        assert!(said(
            Step::Read,
            "remote: HTTP Basic: Access denied\nfatal: Authentication failed for 'https://gitlab.com/acme/lib.git/'"
        )
        .contains("did not accept your sign-in"));
        assert!(said(
            Step::Copy,
            "fatal: unable to access 'https://git.example.com/lib.git/': SSL certificate problem: unable to get local issuer certificate"
        )
        .starts_with("Can't copy the repo to this computer: git can't check gitlab.com's certificate."));
    }

    #[test]
    fn anything_else_keeps_git_s_own_words() {
        assert_eq!(
            said(Step::Fetch, "fatal: the remote end hung up unexpectedly\n"),
            "Can't fetch the repo: git said \"the remote end hung up unexpectedly\". See Details for git's whole message."
        );
        assert_eq!(
            said(Step::Fetch, ""),
            "Can't fetch the repo: git said \"it gave no reason\". See Details for git's whole message."
        );
    }

    #[test]
    fn git_that_is_not_installed_says_where_to_get_it() {
        let problem = git_not_installed(Step::Read);
        assert!(
            problem.summary.starts_with(
                "Can't read the repo: git is not installed on this computer. Install "
            ),
            "{}",
            problem.summary
        );
        assert!(!problem.offline);
    }
}
