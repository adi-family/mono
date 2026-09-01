//! Named ids, and the aliases that keep the old ones answering.
//!
//! Every registry in this store names its entries by their directory: `tools/<id>`,
//! `projects/<id>`, `dashboards/<id>`. That id has always been a free-form string — the store
//! carries `tools/sys-db` beside `tools/ec5bd98c-…` today — so nothing here introduces a new kind
//! of id. What it introduces is the rule for *minting* one: a slug of the name a human already
//! typed, instead of a UUID.
//!
//! The reason is cost, and it is measurable. A UUID is 36 characters of hex that tokenise badly
//! (~23 tokens); an agent definition listing 49 of them spends 1,764 characters of pure identifier
//! in a file read on every launch. `confirm-sync` is 12 characters and two tokens. The second
//! reason is that a published manifest cannot reference what it installs by a machine-local id: a
//! `tools/<uuid>` resolves to nothing on another machine, or worse to something else.
//!
//! **A minted id is unique within its kind on this machine, and it is one path segment.** Not
//! scoped by project: the id *is* a directory name under one module dir, so uniqueness within the
//! kind is what the filesystem already enforces, and a scoped `<project>/<name>` would need a
//! nested directory — breaking [`valid_name`](crate::valid_name), which is the security boundary
//! every store applies before joining an id onto a path. A **published** id carries its
//! publisher (`<publisher>/<name>`, e.g. `adi-family/confirm-sync`) and is resolved to a local id
//! at install time by [`mint`], which is also where a collision with something already installed
//! is settled — the local store never sees the qualified form.
//!
//! **Nothing loses its id.** Renaming an entity records the id it had in the registry's
//! [`Aliases`] index, and every read path resolves through it, so a UUID written down in an agent
//! definition, a `hive.yaml`, a shell history or somebody's notes keeps working. That is not
//! politeness: this store is a live control plane whose ids are cited verbatim in ~75 agent
//! definitions and in generated shims, and an id that stops resolving breaks the machine under its
//! own operator.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::module::Module;

/// The longest slug [`slug`] will produce. Long enough for a phrase somebody typed as a name,
/// short enough that the id stays cheaper than the UUID it replaces — which is the whole point.
pub const MAX_ID: usize = 48;

/// The file a registry keeps its id aliases in, at the root of its module directory
/// (`tools/aliases.toml`, `projects/aliases.toml`, …).
///
/// A file rather than a tombstone directory per old id: every registry lists its entries by
/// reading its module dir, and 44 ghost directories would be 44 things each of those listings has
/// to know to skip. A single file is skipped by the `is_dir` check they already make.
pub const ALIASES_FILE: &str = "aliases.toml";

/// Fold free text into an id: ASCII letters and digits lowercased, every other character a
/// separator, runs of separators collapsed, trimmed, capped at [`MAX_ID`]. `None` when nothing
/// usable is left.
///
/// The alphabet is deliberately narrower than [`valid_name`](crate::valid_name) accepts — no `.`
/// and no `_`. A minted id can then never be `.`, `..`, a dotfile, or something that reads as a
/// file with an extension, so it can never collide with the `aliases.toml` or the `.bin` /
/// `.agent-bin` directories that share the module dir with it.
///
/// A name written entirely in a non-Latin script yields `None`, and the caller falls back to its
/// kind's own word. Inventing a transliteration would produce an id nobody recognises in either
/// language, which is worse than a generic one the operator can rename.
#[must_use]
pub fn slug(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    // Every pushed character is ASCII, so truncating at a byte offset is a char boundary.
    out.truncate(MAX_ID);
    let id = out.trim_matches('-').to_string();
    (!id.is_empty()).then_some(id)
}

/// The first free id in the sequence `base`, `base-2`, `base-3`, … — the one rule every registry
/// settles a collision by, so the answer is the same wherever it is asked and reproducible from
/// the set of ids already taken.
///
/// `taken` must answer for *everything* that occupies the id, aliases included: an id handed out
/// twice would make an old alias resolve to the wrong entity, which is the one failure this whole
/// mechanism exists to prevent.
#[must_use]
pub fn unique_id(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..u32::MAX)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken(candidate))
        // Unreachable short of four billion same-named entries; a duplicate id is still better
        // than a panic in a create path.
        .unwrap_or_else(|| base.to_string())
}

/// Mint the id for a new entity called `name`: [`slug`] of the name, or `fallback` (the kind's own
/// word — `tool`, `project`, `dashboard`) when the name slugs to nothing, made unique by
/// [`unique_id`].
///
/// This is also the install-time half of a published id: strip the publisher, mint the local id
/// from the name, and a manifest's `adi-family/confirm-sync` lands as `confirm-sync` — or
/// `confirm-sync-2` on a machine that already has one, with nothing overwritten.
#[must_use]
pub fn mint(name: &str, fallback: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = slug(name).unwrap_or_else(|| fallback.to_string());
    unique_id(&base, taken)
}

/// One registry's alias index: the ids an entry used to have, each pointing at the id it has now.
///
/// Loaded from `<module>/`[`ALIASES_FILE`]. Read on the *miss* path only — an id that names a live
/// directory is already the answer — so the cost of carrying every old id forever is one small
/// file read, and only when something asked for an id that isn't current.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Aliases {
    /// Old id → current id. A table rather than a bare top-level map so the file says what it is
    /// when somebody opens it.
    #[serde(default)]
    aliases: BTreeMap<String, String>,
}

impl Aliases {
    /// Read `module`'s alias index; a registry that has never renamed anything reads as empty.
    ///
    /// # Errors
    /// [`Error::Io`](crate::Error::Io) on a read failure other than not-found, or
    /// [`Error::Parse`](crate::Error::Parse) on invalid TOML.
    pub fn load(module: &Module) -> Result<Self> {
        module.file::<Self>(ALIASES_FILE).load_or_default()
    }

    /// The id `old` now resolves to, or `None` if nothing was ever renamed from it.
    ///
    /// Only a target that is a safe path segment is returned: the index is a file, and a file can
    /// be hand-edited, so the traversal check has to sit at the point the value leaves this type
    /// rather than at the point it went in.
    #[must_use]
    pub fn target(&self, old: &str) -> Option<&str> {
        self.aliases
            .get(old)
            .map(String::as_str)
            .filter(|to| crate::valid_name(to))
    }

    /// Whether `id` is somebody's old id — the question a mint has to ask, since handing out an id
    /// that still resolves elsewhere is what would break an old reference.
    #[must_use]
    pub fn is_alias(&self, id: &str) -> bool {
        self.aliases.contains_key(id)
    }

    /// Every alias, old id → current id.
    #[must_use]
    pub fn all(&self) -> &BTreeMap<String, String> {
        &self.aliases
    }

    /// Record that `from` is now `to`, so `from` keeps resolving.
    ///
    /// Chains are collapsed rather than followed: an id that pointed at `from` is re-pointed at
    /// `to` here, so one lookup always lands on the live entity and a rename of a rename can never
    /// cost a read more hops than the first one did. And `to` is dropped from the index if it was
    /// itself an alias — an id that names a live entry is that entry, never a redirect to another.
    ///
    /// # Errors
    /// [`Error::Io`](crate::Error::Io) or [`Error::Encode`](crate::Error::Encode) if the index
    /// can't be written, plus everything [`load`](Self::load) can return.
    pub fn record(module: &Module, from: &str, to: &str) -> Result<()> {
        if from == to {
            return Ok(());
        }
        let mut index = Self::load(module)?;
        for target in index.aliases.values_mut() {
            if target == from {
                to.clone_into(target);
            }
        }
        index.aliases.insert(from.to_string(), to.to_string());
        index.aliases.remove(to);
        index.save(module)
    }

    /// Drop every alias that names `id` — its own entry and anything pointing at it — and report
    /// how many went. Called when an entry is deleted: an alias to something that no longer exists
    /// would keep the id reserved against a future mint for no reader's benefit.
    ///
    /// # Errors
    /// As [`record`](Self::record).
    pub fn forget(module: &Module, id: &str) -> Result<usize> {
        let mut index = Self::load(module)?;
        let before = index.aliases.len();
        index.aliases.retain(|from, to| from != id && to != id);
        let dropped = before - index.aliases.len();
        if dropped > 0 {
            index.save(module)?;
        }
        Ok(dropped)
    }

    /// Write the index back to `<module>/`[`ALIASES_FILE`].
    ///
    /// # Errors
    /// [`Error::Io`](crate::Error::Io) or [`Error::Encode`](crate::Error::Encode).
    pub fn save(&self, module: &Module) -> Result<()> {
        module.file::<Self>(ALIASES_FILE).save(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Module {
        let dir = std::env::temp_dir().join(format!(
            "adi-config-ids-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        crate::Config::with_root(dir).module("tools")
    }

    #[test]
    fn slug_folds_a_name_into_one_lowercase_segment() {
        assert_eq!(slug("confirm-sync").as_deref(), Some("confirm-sync"));
        assert_eq!(slug("Deploy Prod").as_deref(), Some("deploy-prod"));
        assert_eq!(slug("  weird!!name  ").as_deref(), Some("weird-name"));
        // Neither `.` nor `_` survives, so a minted id can never read as a dotfile or a filename.
        assert_eq!(
            slug("keep.dots_and-dashes").as_deref(),
            Some("keep-dots-and-dashes")
        );
        assert_eq!(slug(".."), None);
        assert_eq!(slug("!!!"), None);
        assert_eq!(slug("Инструмент"), None);
    }

    #[test]
    fn slug_is_capped_and_never_ends_in_a_separator() {
        let long = slug(&"a b".repeat(60)).expect("slug");
        assert_eq!(long.len(), MAX_ID);
        assert!(!long.ends_with('-'), "{long}");
    }

    #[test]
    fn a_collision_takes_the_next_number_in_order() {
        let taken: std::collections::HashSet<&str> = ["deploy", "deploy-2"].into_iter().collect();
        assert_eq!(unique_id("fresh", |id| taken.contains(id)), "fresh");
        assert_eq!(unique_id("deploy", |id| taken.contains(id)), "deploy-3");
    }

    #[test]
    fn mint_falls_back_to_the_kinds_own_word_for_an_unsluggable_name() {
        assert_eq!(mint("Confirm Sync", "tool", |_| false), "confirm-sync");
        assert_eq!(mint("Инструмент", "tool", |_| false), "tool");
        assert_eq!(mint("Инструмент", "tool", |id| id == "tool"), "tool-2");
    }

    #[test]
    fn an_alias_resolves_and_a_chain_is_collapsed_not_followed() {
        let module = scratch("chain");
        assert!(Aliases::load(&module).expect("empty").all().is_empty());

        Aliases::record(&module, "ec5bd98c", "confirm-sync").expect("first");
        let index = Aliases::load(&module).expect("load");
        assert_eq!(index.target("ec5bd98c"), Some("confirm-sync"));
        assert!(index.is_alias("ec5bd98c"));
        assert_eq!(index.target("nobody"), None);

        // Rename again: the original id must still take one hop, not two.
        Aliases::record(&module, "confirm-sync", "confirm").expect("second");
        let index = Aliases::load(&module).expect("reload");
        assert_eq!(index.target("ec5bd98c"), Some("confirm"));
        assert_eq!(index.target("confirm-sync"), Some("confirm"));

        // Renaming back frees the live id from the index — a live id is never a redirect.
        Aliases::record(&module, "confirm", "confirm-sync").expect("third");
        let index = Aliases::load(&module).expect("reload");
        assert_eq!(index.target("confirm-sync"), None);
        assert_eq!(index.target("ec5bd98c"), Some("confirm-sync"));
        assert_eq!(index.target("confirm"), Some("confirm-sync"));

        let _ = std::fs::remove_dir_all(module.dir());
    }

    #[test]
    fn forget_drops_every_alias_naming_a_deleted_entry() {
        let module = scratch("forget");
        Aliases::record(&module, "old-a", "live").expect("a");
        Aliases::record(&module, "old-b", "other").expect("b");
        assert_eq!(Aliases::forget(&module, "live").expect("forget"), 1);
        let index = Aliases::load(&module).expect("load");
        assert_eq!(index.target("old-a"), None);
        assert_eq!(index.target("old-b"), Some("other"));
        assert_eq!(Aliases::forget(&module, "live").expect("again"), 0);
        let _ = std::fs::remove_dir_all(module.dir());
    }

    #[test]
    fn a_hand_edited_target_that_could_climb_out_is_refused() {
        let module = scratch("traversal");
        module
            .write_raw(ALIASES_FILE, b"[aliases]\n\"old\" = \"../escape\"\n")
            .expect("write");
        assert_eq!(Aliases::load(&module).expect("load").target("old"), None);
        let _ = std::fs::remove_dir_all(module.dir());
    }

    #[test]
    fn recording_an_id_onto_itself_writes_nothing() {
        let module = scratch("noop");
        Aliases::record(&module, "same", "same").expect("noop");
        assert!(!module.raw_path(ALIASES_FILE).exists());
        let _ = std::fs::remove_dir_all(module.dir());
    }
}
