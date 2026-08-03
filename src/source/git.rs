//! Fetching from git, including gists.
//!
//! The command runner is deliberately hostile to interaction. v0 shelled out to
//! `git` with the ambient environment and no timeout, and `skillenv link --quiet`
//! runs from a `chpwd` hook — so a source needing credentials would sit at a
//! password prompt forever, on every directory change, with no output. Every
//! invocation here disables terminal prompting, refuses an askpass helper, and
//! gives up after a deadline.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::manifest::SourceSpec;
use crate::{Result, SkillenvError};

/// How long a single git invocation may take.
///
/// Generous enough for a large shallow clone on a slow link, short enough that an
/// unattended hook is not wedged for the rest of the session.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// How often to check on a running git while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The clone URL for a source, or `None` if it is not fetched over git.
///
/// A gist is a git repository, so `gist:<id>` needs only URL normalization to
/// reuse everything else — including revision pinning, since a gist revision is
/// an ordinary commit sha.
pub(super) fn transport_for(spec: &SourceSpec) -> Option<String> {
    match spec {
        SourceSpec::Gist(id) => Some(format!("https://gist.github.com/{id}.git")),
        SourceSpec::GitHub { owner, repo } => {
            Some(format!("https://github.com/{owner}/{repo}.git"))
        }
        SourceSpec::Git(url) => Some(url.clone()),
        SourceSpec::Local | SourceSpec::Path(_) => None,
    }
}

/// Shallow-fetch `transport` at `git_ref` into `destination`.
///
/// Returns the resolved revision. `destination` must not already exist; callers
/// key the cache on the revision, so a present directory means the work is done.
pub(super) fn fetch_into(
    transport: &str,
    git_ref: Option<&str>,
    destination: &Path,
    not_after: Option<&str>,
) -> Result<String> {
    let dir = destination.display().to_string();
    run(&["init", "--quiet", &dir], None)?;
    run(&["-C", &dir, "remote", "add", "origin", transport], None)?;
    // --depth 1 keeps this to the one commit we are about to check out. When an age
    // limit applies and the tip turns out to be too new, `eligible_revision` deepens
    // from here — so the common case, a tip that is already old enough, costs the
    // same as before.
    run(
        &[
            "-C",
            &dir,
            "fetch",
            "--quiet",
            "--depth",
            "1",
            "origin",
            git_ref.unwrap_or("HEAD"),
        ],
        None,
    )?;

    let wanted = match not_after {
        None => "FETCH_HEAD".to_string(),
        Some(cutoff) => eligible_revision(&dir, git_ref, cutoff, transport)?,
    };
    run(
        &["-C", &dir, "checkout", "--quiet", "--detach", &wanted],
        None,
    )?;
    let revision = run(&["-C", &dir, "rev-parse", "HEAD"], None)?;
    Ok(revision.trim().to_string())
}

/// How far back to reach when the tip is too new, and how many times.
///
/// Deepening is a round trip each time, so it doubles rather than creeping. The cap
/// exists so a repository whose whole history is newer than the cutoff reports that
/// instead of walking to the root commit over many fetches.
const DEEPEN_STEPS: [&str; 4] = ["16", "64", "256", "1024"];

/// The newest revision at or before `cutoff`, deepening history until one is found.
///
/// `--shallow-since` cannot do this: it fetches commits *newer* than a date and stops
/// short of the boundary, so the one commit we are looking for is exactly the one it
/// leaves out.
fn eligible_revision(
    dir: &str,
    git_ref: Option<&str>,
    cutoff: &str,
    transport: &str,
) -> Result<String> {
    let reference = git_ref.unwrap_or("HEAD");
    let before = format!("--before={cutoff}");

    for depth in DEEPEN_STEPS {
        let found = run(&["-C", dir, "rev-list", "-1", &before, "FETCH_HEAD"], None)?;
        let found = found.trim();
        if !found.is_empty() {
            return Ok(found.to_string());
        }

        let before_deepen = count_commits(dir)?;
        run(
            &[
                "-C", dir, "fetch", "--quiet", "--deepen", depth, "origin", reference,
            ],
            None,
        )?;
        // Nothing new arrived, so the history is exhausted and no commit is old
        // enough. Deepening again would be the same round trip for the same answer.
        if count_commits(dir)? == before_deepen {
            break;
        }
    }

    Err(SkillenvError::NoRevisionOldEnough {
        transport: transport.to_string(),
        reference: reference.to_string(),
        cutoff: cutoff.to_string(),
    })
}

fn count_commits(dir: &str) -> Result<usize> {
    Ok(
        run(&["-C", dir, "rev-list", "--count", "FETCH_HEAD"], None)?
            .trim()
            .parse()
            .unwrap_or(0),
    )
}

/// The revision `git_ref` currently points at on the remote, without fetching.
///
/// This is what makes a non-mutating staleness check possible. v0 had no such
/// path: `update` always fetched, wiped the install root, and rewrote the lock,
/// so there was no way to ask "is anything out of date" and get an answer back.
pub(super) fn remote_revision(transport: &str, git_ref: Option<&str>) -> Result<String> {
    let reference = git_ref.unwrap_or("HEAD");
    let output = run(&["ls-remote", transport, reference], None)?;

    // `ls-remote` prints `<sha>\t<ref>` per match. An exact ref may also report
    // its dereferenced tag as `<ref>^{}`, which is the commit we want.
    let mut fallback = None;
    for line in output.lines() {
        let Some((sha, name)) = line.split_once('\t') else {
            continue;
        };
        if name.ends_with("^{}") {
            return Ok(sha.trim().to_string());
        }
        fallback.get_or_insert(sha.trim().to_string());
    }

    fallback.ok_or_else(|| SkillenvError::UnknownRemoteRef {
        transport: transport.to_string(),
        reference: reference.to_string(),
    })
}

/// Run `git` with prompting disabled and a deadline.
/// Run git and take its output, whatever it exits with.
///
/// For commands where a non-zero exit is an answer rather than a failure — `git
/// diff` returns 1 to mean "they differ" — so treating it as an error would discard
/// the very output that was wanted.
pub(crate) fn run_reporting_status(args: &[&str], cwd: Option<&Path>) -> Result<String> {
    run_raw(args, cwd).map(|(_success, stdout, _stderr)| stdout)
}

pub(super) fn run(args: &[&str], cwd: Option<&Path>) -> Result<String> {
    let (success, stdout, stderr) = run_raw(args, cwd)?;
    if success {
        Ok(stdout)
    } else {
        Err(SkillenvError::CommandFailed {
            program: "git".to_string(),
            cwd: cwd.map(Path::to_path_buf),
            stderr: stderr.trim().to_string(),
        })
    }
}

/// Spawn git with everything interactive disabled, and a timeout.
fn run_raw(args: &[&str], cwd: Option<&Path>) -> Result<(bool, String, String)> {
    let mut command = Command::new("git");
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    // Anything that could block waiting for a human is turned off. Without this
    // an unattended hook can hang indefinitely on a private repository.
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GCM_INTERACTIVE", "never")
        // Keep messages parseable regardless of the user's locale.
        .env("LC_ALL", "C");

    // An askpass helper is what a *graphical* credential prompt comes through, so
    // pointing it at a no-op closes that path too. Only set when the binary really
    // exists: `/bin/true` is absent on macOS, and a missing helper makes git print
    // `cannot exec` noise that buries the real reason a fetch failed.
    if let Some(no_op) = no_op_binary() {
        command.env("GIT_ASKPASS", no_op).env("SSH_ASKPASS", no_op);
    }

    let mut child = command
        .spawn()
        .map_err(|source| SkillenvError::RunCommand {
            program: "git".to_string(),
            cwd: cwd.map(Path::to_path_buf),
            source,
        })?;

    // Drained on their own threads, started before the wait loop. Reading only after
    // the process exits deadlocks as soon as output exceeds the pipe buffer: git
    // blocks writing, never exits, and the timeout below turns a fast command into a
    // two-minute failure. `git diff` output is unbounded, so this is reachable.
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(pipe) = stdout_pipe.as_mut() {
            let _ = pipe.read_to_string(&mut text);
        }
        text
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(pipe) = stderr_pipe.as_mut() {
            let _ = pipe.read_to_string(&mut text);
        }
        text
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= GIT_TIMEOUT {
                    // Best effort: if the kill fails there is nothing better to do
                    // than report the timeout.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SkillenvError::CommandTimedOut {
                        program: format!("git {}", args.join(" ")),
                        seconds: GIT_TIMEOUT.as_secs(),
                    });
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(source) => {
                return Err(SkillenvError::RunCommand {
                    program: "git".to_string(),
                    cwd: cwd.map(Path::to_path_buf),
                    source,
                });
            }
        }
    };

    // The readers end when the pipes close, which the exit above guarantees.
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();

    Ok((status.success(), stdout, stderr))
}

/// A binary that exits successfully and prints nothing, if one is where we expect.
///
/// `/usr/bin/true` exists on both macOS and Linux; `/bin/true` only on Linux.
fn no_op_binary() -> Option<&'static str> {
    ["/usr/bin/true", "/bin/true"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
}

/// Resolve a subdirectory within a fetched tree, refusing to leave it.
///
/// v0 joined the path with no checks, so `subdir = "../../etc"` would read
/// outside the checkout entirely. Validation happens before any git runs.
pub(super) fn resolve_subdir(root: &Path, subdir: Option<&Path>) -> Result<PathBuf> {
    let Some(subdir) = subdir else {
        return Ok(root.to_path_buf());
    };
    validate_subdir(subdir)?;
    Ok(root.join(subdir))
}

/// Reject a subdirectory that could escape the tree it is relative to.
pub(super) fn validate_subdir(subdir: &Path) -> Result<()> {
    use std::path::Component;

    let invalid = |message: &str| {
        Err(SkillenvError::InvalidSource {
            input: subdir.display().to_string(),
            message: message.to_string(),
        })
    };

    if subdir.is_absolute() {
        return invalid("subdirectory must be relative to the source root");
    }
    for component in subdir.components() {
        match component {
            Component::ParentDir => {
                return invalid(
                    "subdirectory must not contain '..'; it would read outside the source",
                );
            }
            Component::Prefix(_) | Component::RootDir => {
                return invalid("subdirectory must be a plain relative path");
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gist_becomes_a_clone_url_so_the_git_path_is_reused() {
        assert_eq!(
            transport_for(&SourceSpec::Gist("fd287c31".to_string())).as_deref(),
            Some("https://gist.github.com/fd287c31.git")
        );
    }

    #[test]
    fn github_and_plain_git_urls_map_through() {
        assert_eq!(
            transport_for(&SourceSpec::GitHub {
                owner: "igtm".to_string(),
                repo: "kinko".to_string(),
            })
            .as_deref(),
            Some("https://github.com/igtm/kinko.git")
        );
        assert_eq!(
            transport_for(&SourceSpec::Git("git@github.com:o/r.git".to_string())).as_deref(),
            Some("git@github.com:o/r.git")
        );
    }

    #[test]
    fn local_sources_have_no_transport() {
        assert!(transport_for(&SourceSpec::Local).is_none());
        assert!(transport_for(&SourceSpec::Path(PathBuf::from("../x"))).is_none());
    }

    /// The check has to happen before any git command runs, not after the join.
    #[test]
    fn a_traversing_subdir_is_refused() {
        for bad in ["../etc", "a/../../etc", "/etc"] {
            let error = validate_subdir(Path::new(bad)).unwrap_err().to_string();
            assert!(error.contains("subdirectory"), "{bad:?} gave {error:?}");
        }
    }

    #[test]
    fn an_ordinary_subdir_is_accepted() -> Result<()> {
        validate_subdir(Path::new("skills/writing"))?;
        assert_eq!(
            resolve_subdir(Path::new("/tmp/co"), Some(Path::new("skills")))?,
            PathBuf::from("/tmp/co/skills")
        );
        assert_eq!(
            resolve_subdir(Path::new("/tmp/co"), None)?,
            PathBuf::from("/tmp/co")
        );
        Ok(())
    }

    /// Exercises the real runner against a local repository, so the hardened
    /// environment and the ls-remote parsing are covered without network access.
    #[test]
    fn fetches_and_inspects_a_local_repository() -> Result<()> {
        let origin = tempfile::tempdir().unwrap();
        let origin_path = origin.path().join("repo");
        std::fs::create_dir_all(&origin_path).unwrap();
        let dir = origin_path.display().to_string();
        run(&["init", "--quiet", "--initial-branch", "main", &dir], None)?;
        std::fs::write(origin_path.join("SKILL.md"), "body\n").unwrap();
        run(&["-C", &dir, "add", "."], None)?;
        run(
            &[
                "-C",
                &dir,
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "commit",
                "--quiet",
                "-m",
                "initial",
            ],
            None,
        )?;

        let head = run(&["-C", &dir, "rev-parse", "HEAD"], None)?
            .trim()
            .to_string();

        // ls-remote reports the same revision without fetching anything.
        assert_eq!(remote_revision(&dir, Some("main"))?, head);

        let work = tempfile::tempdir().unwrap();
        let destination = work.path().join("checkout");
        std::fs::create_dir_all(&destination).unwrap();
        assert_eq!(fetch_into(&dir, Some("main"), &destination, None)?, head);
        assert_eq!(
            std::fs::read_to_string(destination.join("SKILL.md")).unwrap(),
            "body\n"
        );
        Ok(())
    }

    #[test]
    fn an_unknown_ref_is_reported_by_name() -> Result<()> {
        let origin = tempfile::tempdir().unwrap();
        let origin_path = origin.path().join("repo");
        std::fs::create_dir_all(&origin_path).unwrap();
        let dir = origin_path.display().to_string();
        run(&["init", "--quiet", "--initial-branch", "main", &dir], None)?;
        std::fs::write(origin_path.join("SKILL.md"), "body\n").unwrap();
        run(&["-C", &dir, "add", "."], None)?;
        run(
            &[
                "-C",
                &dir,
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "commit",
                "--quiet",
                "-m",
                "initial",
            ],
            None,
        )?;

        let error = remote_revision(&dir, Some("no-such-branch"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("no-such-branch"), "unexpected: {error}");
        Ok(())
    }
}
