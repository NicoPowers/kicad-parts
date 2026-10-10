//! A local copy of every part's current revision, kept in step with the
//! team server's Changes.
//!
//! Changes are numbered by `seq` without gaps. The replica applies them
//! strictly in that order and remembers the last one it applied (its
//! checkpoint). A Change can arrive twice, through a realtime notification
//! and through catching up after a disconnect; the second copy is ignored,
//! so nothing is missed or applied twice. This follows
//! `collaboration::Replica`, which does the same for the provisional
//! `collab_*` records.

use crate::part::PartDetails;
use crate::pocketbase::{StoredChange, StoredPartRevision, changes_since, revisions_in_change};
use collaboration::{ApplyResult, CollabError, CollabResult, PocketBaseClient};
use std::collections::BTreeMap;

/// A part's current revision as the replica knows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplicaPart {
    pub revision_id: String,
    pub number: u32,
    pub snapshot_sha256: String,
    /// Its part details, when the snapshot parses.
    pub details: Option<PartDetails>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CatalogReplica {
    checkpoint: u64,
    /// By part ID.
    parts: BTreeMap<String, ReplicaPart>,
    /// Part IDs by IPN.
    ipns: BTreeMap<String, String>,
    applied: Vec<u64>,
}

impl CatalogReplica {
    pub fn new() -> Self {
        Self::default()
    }

    /// The `seq` of the last applied Change (0 before any).
    pub fn checkpoint(&self) -> u64 {
        self.checkpoint
    }

    /// Current revisions by part ID.
    pub fn parts(&self) -> &BTreeMap<String, ReplicaPart> {
        &self.parts
    }

    /// The current revision of the part with `ipn`, if known.
    pub fn part_by_ipn(&self, ipn: &str) -> Option<&ReplicaPart> {
        self.parts.get(self.ipns.get(ipn)?)
    }

    /// Every applied `seq`, in the order applied.
    pub fn applied_sequences(&self) -> &[u64] {
        &self.applied
    }

    /// Applies `change`, whose part revisions are `revisions`, if it is the
    /// next one after the checkpoint.
    pub fn apply(
        &mut self,
        change: &StoredChange,
        revisions: &[StoredPartRevision],
    ) -> ApplyResult {
        if change.seq <= self.checkpoint {
            return ApplyResult::Duplicate;
        }
        if change.seq != self.checkpoint + 1 {
            return ApplyResult::Gap {
                checkpoint: self.checkpoint,
                received: change.seq,
            };
        }
        for revision in revisions
            .iter()
            .filter(|revision| revision.change == change.id)
        {
            let newer = self
                .parts
                .get(&revision.part)
                .is_none_or(|known| known.number < revision.number);
            if newer {
                let details: Option<PartDetails> = serde_json::from_str(&revision.snapshot).ok();
                if let Some(details) = &details {
                    self.ipns.insert(details.ipn.clone(), revision.part.clone());
                }
                self.parts.insert(
                    revision.part.clone(),
                    ReplicaPart {
                        revision_id: revision.id.clone(),
                        number: revision.number,
                        snapshot_sha256: revision.snapshot_sha256.clone(),
                        details: details.clone(),
                    },
                );
            }
        }
        self.checkpoint = change.seq;
        self.applied.push(change.seq);
        ApplyResult::Applied
    }

    /// Fetches and applies every Change after the checkpoint. Returns how
    /// many were applied.
    pub fn catch_up(&mut self, client: &PocketBaseClient) -> CollabResult<usize> {
        let mut applied = 0;
        for change in changes_since(client, self.checkpoint)? {
            let revisions = revisions_in_change(client, &change.id)?;
            match self.apply(&change, &revisions) {
                ApplyResult::Applied => applied += 1,
                ApplyResult::Duplicate => {}
                ApplyResult::Gap {
                    checkpoint,
                    received,
                } => {
                    return Err(CollabError::Protocol(format!(
                        "the Changes skip from {checkpoint} to {received}"
                    )));
                }
            }
        }
        Ok(applied)
    }

    /// Applies a Change that arrived as a realtime notification. A gap
    /// means some Changes were missed, so it catches up first, which also
    /// covers this one.
    pub fn receive(
        &mut self,
        change: &StoredChange,
        client: &PocketBaseClient,
    ) -> CollabResult<ApplyResult> {
        if change.seq <= self.checkpoint {
            return Ok(ApplyResult::Duplicate);
        }
        if change.seq != self.checkpoint + 1 {
            self.catch_up(client)?;
            return Ok(if change.seq <= self.checkpoint {
                ApplyResult::Applied
            } else {
                ApplyResult::Gap {
                    checkpoint: self.checkpoint,
                    received: change.seq,
                }
            });
        }
        let revisions = revisions_in_change(client, &change.id)?;
        Ok(self.apply(change, &revisions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(seq: u64) -> StoredChange {
        StoredChange {
            id: format!("change{seq}"),
            seq,
            kind: "edit".into(),
            reason: "r".into(),
            correlation_id: format!("cmd-{seq}"),
            actor: "users:a".into(),
            actor_name: "A".into(),
            created: String::new(),
            source_commit: String::new(),
            sources: "{}".into(),
            summary: "{}".into(),
        }
    }

    fn revision(seq: u64, part: &str, number: u32) -> StoredPartRevision {
        StoredPartRevision {
            id: format!("{part}-{number}"),
            part: part.into(),
            number,
            predecessor: String::new(),
            change: format!("change{seq}"),
            snapshot: "{}".into(),
            snapshot_sha256: format!("{number:064}"),
            source: "{}".into(),
            cad_item_revisions: vec![],
        }
    }

    #[test]
    fn changes_apply_in_order_once() {
        let mut replica = CatalogReplica::new();
        assert_eq!(
            replica.apply(&change(1), &[revision(1, "p", 1), revision(1, "q", 1)]),
            ApplyResult::Applied
        );
        assert_eq!(
            replica.apply(&change(3), &[revision(3, "p", 3)]),
            ApplyResult::Gap {
                checkpoint: 1,
                received: 3
            }
        );
        assert_eq!(
            replica.apply(&change(2), &[revision(2, "p", 2)]),
            ApplyResult::Applied
        );
        assert_eq!(
            replica.apply(&change(2), &[revision(2, "p", 2)]),
            ApplyResult::Duplicate
        );
        assert_eq!(replica.checkpoint(), 2);
        assert_eq!(replica.parts()["p"].number, 2);
        assert_eq!(replica.parts()["q"].number, 1);
        assert_eq!(replica.applied_sequences(), [1, 2]);
    }
}
