//! The flat catalog: every skill the manifest resolves to, keyed by a unique id.
//!
//! Flatness is the point. v0 discovered skills by walking
//! `skillenv/{default,local,profiles/<name>}` at a fixed depth and keyed
//! duplicate detection on `(scope, id)`, so the same skill declared under two
//! scopes was two different skills that both deployed. Adding a scope meant
//! editing thirteen places, because scope identity was a directory name that had
//! to round-trip through three separate string encodings.
//!
//! Here there is one namespace. Two declarations of one id are an error, found
//! while resolving rather than part-way through writing files.
//!
//! Nothing walks a catalog yet — `deploy` is the first consumer, and this allow
//! goes away with it.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::manifest::{
    DeployRule, Manifest, SkillEntry, SkillId, SkillSelection, SourceEntry, SourceSpec,
};
use crate::{Result, SkillenvError};

/// Where the skills a manifest declares directly are kept.
pub(crate) const LOCAL_SKILLS_DIR: &str = "skills";

/// One resolved skill: what it is, where its bytes come from, how it is labelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: SkillId,
    pub source: SourceSpec,
    /// The `[[source]]` that contributed it, when it did not come from a
    /// `[[skill]]` declaration.
    pub source_name: Option<String>,
    /// Set on a source, so every skill it contributes inherits it.
    pub git_ref: Option<String>,
    /// Supplied in the manifest for sources that carry no frontmatter, e.g. a
    /// gist. `None` means "take it from the skill's own frontmatter".
    pub description: Option<String>,
    pub labels: Vec<String>,
}

impl CatalogEntry {
    /// Absolute directory holding this skill's bytes, for a local source.
    ///
    /// Remote sources live in the cache and are not resolvable until fetched, so
    /// this returns `None` for them rather than guessing a path.
    pub fn local_dir(&self, manifest_root: &Path) -> Option<PathBuf> {
        match &self.source {
            SourceSpec::Local => Some(manifest_root.join(LOCAL_SKILLS_DIR).join(self.id.as_str())),
            SourceSpec::Path(path) => Some(if path.is_absolute() {
                path.clone()
            } else {
                manifest_root.join(path)
            }),
            _ => None,
        }
    }

    /// Whether this skill has to be fetched before it can be deployed.
    pub fn needs_fetch(&self) -> bool {
        !matches!(self.source, SourceSpec::Local)
    }
}

/// Everything a manifest resolves to, plus the rules that place it.
#[derive(Debug, Clone)]
pub struct Catalog {
    /// Directory holding `skillenv.toml`. Local sources resolve against it.
    pub root: PathBuf,
    /// Entries in a stable order, keyed by id.
    pub entries: BTreeMap<SkillId, CatalogEntry>,
    /// Sources whose skill set is only known after fetching.
    pub wildcard_sources: Vec<SourceEntry>,
    pub deploys: Vec<DeployRule>,
}

impl Catalog {
    /// Resolve a manifest into a catalog.
    ///
    /// Only what the manifest states is resolved here; a `[[source]]` with
    /// `skills = "*"` contributes nothing yet, because its members are whatever
    /// the source turns out to contain. They join the catalog after a fetch,
    /// through [`Catalog::admit`], which is also where their ids get checked for
    /// collisions.
    pub fn resolve(manifest: &Manifest, root: &Path) -> Result<Self> {
        let mut entries: BTreeMap<SkillId, CatalogEntry> = BTreeMap::new();
        let mut folded: BTreeMap<String, SkillId> = BTreeMap::new();

        for skill in &manifest.skills {
            insert(&mut entries, &mut folded, entry_from_skill(skill))?;
        }

        let mut wildcard_sources = Vec::new();
        for source in &manifest.sources {
            match &source.skills {
                SkillSelection::Explicit(ids) => {
                    for id in ids {
                        insert(
                            &mut entries,
                            &mut folded,
                            entry_from_source(source, id.clone()),
                        )?;
                    }
                }
                SkillSelection::All => wildcard_sources.push(source.clone()),
            }
        }

        Ok(Self {
            root: root.to_path_buf(),
            entries,
            wildcard_sources,
            deploys: manifest.deploys.clone(),
        })
    }

    /// Add a skill discovered by fetching a wildcard source.
    ///
    /// Kept separate from `resolve` so a collision between a declared skill and a
    /// newly-discovered one is reported the same way as any other duplicate,
    /// rather than one silently overwriting the other.
    pub fn admit(&mut self, source: &SourceEntry, id: SkillId) -> Result<()> {
        let mut folded = self.folded_index();
        insert(
            &mut self.entries,
            &mut folded,
            entry_from_source(source, id),
        )
    }

    pub fn get(&self, id: &SkillId) -> Option<&CatalogEntry> {
        self.entries.get(id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.values()
    }

    /// Skills a rule selects, in catalog order.
    pub fn selected_by(&self, rule: &DeployRule) -> Vec<&CatalogEntry> {
        self.entries
            .values()
            .filter(|entry| rule.selects(&entry.id, &entry.labels))
            .collect()
    }

    /// Skills selected by any of several rules, deduplicated.
    ///
    /// Used where rules share a target directory: their selections are unioned so
    /// the two do not take turns removing each other's work.
    pub fn selected_by_any<'a>(
        &'a self,
        rules: impl IntoIterator<Item = &'a DeployRule>,
    ) -> Vec<&'a CatalogEntry> {
        let mut chosen: BTreeMap<&SkillId, &CatalogEntry> = BTreeMap::new();
        for rule in rules {
            for entry in self.selected_by(rule) {
                chosen.insert(&entry.id, entry);
            }
        }
        chosen.into_values().collect()
    }

    fn folded_index(&self) -> BTreeMap<String, SkillId> {
        self.entries
            .keys()
            .map(|id| (id.as_str().to_lowercase(), id.clone()))
            .collect()
    }
}

fn entry_from_skill(skill: &SkillEntry) -> CatalogEntry {
    CatalogEntry {
        id: skill.id.clone(),
        source: skill.source.clone(),
        source_name: None,
        git_ref: None,
        description: skill.description.clone(),
        labels: skill.labels.clone(),
    }
}

fn entry_from_source(source: &SourceEntry, id: SkillId) -> CatalogEntry {
    CatalogEntry {
        id,
        source: source.from.clone(),
        source_name: Some(source.name.clone()),
        git_ref: source.git_ref.clone(),
        description: None,
        labels: source.labels.clone(),
    }
}

/// Insert an entry, refusing a second declaration of the same id.
///
/// The fold key is lowercase because macOS and Windows are case-insensitive by
/// default: `Foo` and `foo` are distinct strings that name one directory, so
/// accepting both would produce a collision only at write time.
fn insert(
    entries: &mut BTreeMap<SkillId, CatalogEntry>,
    folded: &mut BTreeMap<String, SkillId>,
    entry: CatalogEntry,
) -> Result<()> {
    let key = entry.id.as_str().to_lowercase();
    if let Some(existing) = folded.get(&key) {
        let first = entries
            .get(existing)
            .map(describe_origin)
            .unwrap_or_else(|| "an earlier declaration".to_string());
        return Err(SkillenvError::DuplicateSkillId {
            id: entry.id.to_string(),
            first,
            second: describe_origin(&entry),
        });
    }
    folded.insert(key, entry.id.clone());
    entries.insert(entry.id.clone(), entry);
    Ok(())
}

/// How to describe where an entry came from, for a collision message.
fn describe_origin(entry: &CatalogEntry) -> String {
    match &entry.source_name {
        Some(name) => format!("source '{name}'"),
        None => "a [[skill]] declaration".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::TargetScope;

    fn catalog(toml: &str) -> Result<Catalog> {
        let manifest = Manifest::parse(toml, Path::new("skillenv.toml"))?;
        Catalog::resolve(&manifest, Path::new("/work/dotfiles"))
    }

    fn id(raw: &str) -> SkillId {
        SkillId::parse(raw).expect("test id should be valid")
    }

    #[test]
    fn resolves_declared_skills_and_explicit_source_members() -> Result<()> {
        let catalog = catalog(
            r#"
[[skill]]
name = "draft-pr"
source = "local"
labels = ["tools"]

[[source]]
name = "igtm-skills"
from = "github:igtm/skills"
ref = "main"
skills = ["user-context", "visual-explainer"]
labels = ["upstream"]
"#,
        )?;

        assert_eq!(catalog.len(), 3);
        let local = catalog.get(&id("draft-pr")).unwrap();
        assert_eq!(local.source, SourceSpec::Local);
        assert_eq!(local.source_name, None);

        let from_source = catalog.get(&id("user-context")).unwrap();
        assert_eq!(from_source.source_name.as_deref(), Some("igtm-skills"));
        // Labels and ref come from the source, so every member inherits them.
        assert_eq!(from_source.labels, vec!["upstream".to_string()]);
        assert_eq!(from_source.git_ref.as_deref(), Some("main"));
        Ok(())
    }

    /// A wildcard source contributes nothing until it has been fetched, because
    /// its membership is whatever the source turns out to hold.
    #[test]
    fn a_wildcard_source_is_deferred_rather_than_guessed() -> Result<()> {
        let catalog = catalog(
            "[[source]]\nname = \"vercel\"\nfrom = \"github:vercel-labs/agent-skills\"\n\
             skills = \"*\"\nlabels = [\"design\"]\n",
        )?;
        assert!(catalog.is_empty());
        assert_eq!(catalog.wildcard_sources.len(), 1);

        // Fetching admits its members, and they inherit the source's labels.
        let mut catalog = catalog;
        let source = catalog.wildcard_sources[0].clone();
        catalog.admit(&source, id("frontend-design"))?;
        assert_eq!(catalog.len(), 1);
        assert_eq!(
            catalog.get(&id("frontend-design")).unwrap().labels,
            vec!["design".to_string()]
        );
        Ok(())
    }

    /// A newly-discovered skill colliding with a declared one is an error, not a
    /// silent overwrite.
    #[test]
    fn admitting_a_colliding_id_is_refused_and_names_both_origins() -> Result<()> {
        let mut catalog = catalog(
            "[[skill]]\nname = \"handoff\"\nsource = \"local\"\n\
             [[source]]\nname = \"upstream\"\nfrom = \"github:o/r\"\nskills = \"*\"\n",
        )?;
        let source = catalog.wildcard_sources[0].clone();
        let error = catalog
            .admit(&source, id("handoff"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("handoff"), "unexpected: {error}");
        assert!(
            error.contains("[[skill]]"),
            "should name the first: {error}"
        );
        assert!(
            error.contains("upstream"),
            "should name the second: {error}"
        );
        Ok(())
    }

    #[test]
    fn a_local_skill_resolves_under_the_manifests_skills_directory() -> Result<()> {
        let catalog = catalog("[[skill]]\nname = \"draft-pr\"\nsource = \"local\"\n")?;
        let entry = catalog.get(&id("draft-pr")).unwrap();
        assert_eq!(
            entry.local_dir(&catalog.root),
            Some(PathBuf::from("/work/dotfiles/skills/draft-pr"))
        );
        assert!(!entry.needs_fetch());
        Ok(())
    }

    /// A remote source has no on-disk location until it is fetched, so asking for
    /// one returns nothing rather than a path that does not exist.
    #[test]
    fn a_remote_skill_has_no_local_directory_before_fetching() -> Result<()> {
        let catalog =
            catalog("[[source]]\nname = \"s\"\nfrom = \"github:o/r\"\nskills = [\"kinko\"]\n")?;
        let entry = catalog.get(&id("kinko")).unwrap();
        assert_eq!(entry.local_dir(&catalog.root), None);
        assert!(entry.needs_fetch());
        Ok(())
    }

    #[test]
    fn a_relative_path_source_resolves_against_the_manifest() -> Result<()> {
        let catalog = catalog("[[skill]]\nname = \"shared\"\nsource = \"path:../shared\"\n")?;
        assert_eq!(
            catalog.get(&id("shared")).unwrap().local_dir(&catalog.root),
            Some(PathBuf::from("/work/dotfiles/../shared"))
        );
        Ok(())
    }

    #[test]
    fn selection_follows_labels_and_ids() -> Result<()> {
        let catalog = catalog(
            r#"
[[skill]]
name = "japanese-tech-writing"
source = "local"
labels = ["writing"]

[[skill]]
name = "draft-pr"
source = "local"
labels = ["tools"]

[[deploy]]
target = "claude:home"
include = ["writing"]
"#,
        )?;
        let selected = catalog.selected_by(&catalog.deploys[0]);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, id("japanese-tech-writing"));
        Ok(())
    }

    /// Rules sharing a directory have their selections unioned, so neither
    /// removes what the other placed.
    #[test]
    fn selections_from_several_rules_are_unioned_without_duplicates() -> Result<()> {
        let catalog = catalog(
            r#"
[[skill]]
name = "a"
source = "local"
labels = ["one", "both"]

[[skill]]
name = "b"
source = "local"
labels = ["two", "both"]

[[deploy]]
target = "claude:home"
include = ["one"]

[[deploy]]
target = "claude:home"
include = ["both"]
"#,
        )?;
        let union = catalog.selected_by_any(&catalog.deploys);
        let ids: Vec<_> = union.iter().map(|entry| entry.id.to_string()).collect();
        assert_eq!(ids, vec!["a", "b"], "each skill should appear once");
        Ok(())
    }

    #[test]
    fn entries_come_back_in_a_stable_order() -> Result<()> {
        let catalog = catalog(
            "[[skill]]\nname = \"zeta\"\nsource = \"local\"\n\
             [[skill]]\nname = \"alpha\"\nsource = \"local\"\n",
        )?;
        let ids: Vec<_> = catalog.iter().map(|entry| entry.id.to_string()).collect();
        assert_eq!(ids, vec!["alpha", "zeta"]);
        Ok(())
    }

    #[test]
    fn deploy_rules_are_carried_through() -> Result<()> {
        let catalog = catalog("[[deploy]]\ntarget = \"claude:repo\"\ninclude = [\"*\"]\n")?;
        assert_eq!(catalog.deploys.len(), 1);
        assert_eq!(catalog.deploys[0].target.scope, TargetScope::Repo);
        Ok(())
    }
}
