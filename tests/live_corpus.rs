//! The safeguard must not block the skills already in use.
//!
//! These are real files, and they exercise exactly the shapes a naive scanner
//! gets wrong: a secrets manager documenting `.env` and `~/.ssh`, a Figma skill
//! fetching from a loopback MCP server and documenting a piped installer, a PR
//! skill running `gh` and `git push`. If any of them produced a critical finding
//! the default policy would refuse to deploy it, and the feature would be turned
//! off — which is worse than not shipping it.
//!
//! The corpus lives under fixtures/live_corpus/ so the guarantee is checked in
//! CI rather than resting on a one-off manual run.

use std::fs;
use std::path::Path;

#[test]
fn no_skill_in_the_live_corpus_produces_a_critical_finding() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/live_corpus");
    let entries: Vec<_> = fs::read_dir(&corpus)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", corpus.display()))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
        .collect();

    assert!(
        entries.len() >= 16,
        "expected the whole corpus, found {} files",
        entries.len()
    );

    let mut offenders = Vec::new();
    for entry in entries {
        let text = fs::read_to_string(entry.path()).unwrap();
        for finding in skillenv::scan_skill_text(&text) {
            if finding.blocks_by_default() {
                offenders.push(format!(
                    "{}: {finding}",
                    entry.file_name().to_string_lossy()
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "the safeguard would refuse to deploy skills that are already in use:\n  {}",
        offenders.join("\n  ")
    );
}
