//! A local copy of the collaborative records, kept in step with the
//! server's change log.

use crate::client::PocketBaseClient;
use crate::contract::ChangeEntry;
use crate::{CollabError, CollabResult};
use std::collections::BTreeMap;

/// A record as the replica knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicaRecord {
    pub record_id: String,
    pub revision: u64,
    pub title: String,
    pub content: String,
}

/// What [`Replica::apply`] did with a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyResult {
    Applied,
    /// Already applied (its seq is at or below the checkpoint); ignored.
    Duplicate,
    /// Changes between the checkpoint and this one are missing; nothing
    /// was applied. Catch up from the checkpoint.
    Gap {
        checkpoint: u64,
        received: u64,
    },
}

/// Applies changes strictly in `seq` order. The checkpoint is the last
/// applied `seq`; after a disconnect, [`Replica::catch_up`] fetches
/// everything after it, so no change is missed and none is applied twice,
/// whichever way (realtime or catch-up) it arrives first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Replica {
    checkpoint: u64,
    records: BTreeMap<String, ReplicaRecord>,
    applied: Vec<u64>,
}

impl Replica {
    pub fn new() -> Self {
        Self::default()
    }

    /// The last applied change-log `seq` (0 before any).
    pub fn checkpoint(&self) -> u64 {
        self.checkpoint
    }

    /// Records by key.
    pub fn records(&self) -> &BTreeMap<String, ReplicaRecord> {
        &self.records
    }

    /// Every applied `seq`, in the order applied.
    pub fn applied_sequences(&self) -> &[u64] {
        &self.applied
    }

    pub fn apply(&mut self, change: &ChangeEntry) -> ApplyResult {
        if change.seq <= self.checkpoint {
            return ApplyResult::Duplicate;
        }
        if change.seq != self.checkpoint + 1 {
            return ApplyResult::Gap {
                checkpoint: self.checkpoint,
                received: change.seq,
            };
        }
        self.records.insert(
            change.record_key.clone(),
            ReplicaRecord {
                record_id: change.record.clone(),
                revision: change.revision,
                title: change.title.clone(),
                content: change.content.clone(),
            },
        );
        self.checkpoint = change.seq;
        self.applied.push(change.seq);
        ApplyResult::Applied
    }

    /// Fetches and applies every change after the checkpoint. Returns how
    /// many were applied.
    pub fn catch_up(&mut self, client: &PocketBaseClient) -> CollabResult<usize> {
        let mut applied = 0;
        for change in client.changes_since(self.checkpoint)? {
            match self.apply(&change) {
                ApplyResult::Applied => applied += 1,
                ApplyResult::Duplicate => {}
                ApplyResult::Gap {
                    checkpoint,
                    received,
                } => {
                    return Err(CollabError::Protocol(format!(
                        "the change log skips from {checkpoint} to {received}"
                    )));
                }
            }
        }
        Ok(applied)
    }

    /// Applies a change that arrived as a notification; on a gap, catches
    /// up from the server first (which also covers this change).
    pub fn receive(
        &mut self,
        change: &ChangeEntry,
        client: &PocketBaseClient,
    ) -> CollabResult<ApplyResult> {
        match self.apply(change) {
            ApplyResult::Gap { .. } => {
                self.catch_up(client)?;
                Ok(self.apply(change))
            }
            result => Ok(result),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(seq: u64, key: &str, revision: u64, title: &str) -> ChangeEntry {
        ChangeEntry {
            id: format!("c{seq}"),
            seq,
            record: format!("id-{key}"),
            record_key: key.into(),
            revision,
            correlation_id: format!("cmd-{seq}"),
            actor: "u".into(),
            title: title.into(),
            content: String::new(),
        }
    }

    #[test]
    fn changes_apply_in_sequence_and_duplicates_are_ignored() {
        let mut replica = Replica::new();
        assert_eq!(
            replica.apply(&change(1, "a", 1, "A1")),
            ApplyResult::Applied
        );
        assert_eq!(
            replica.apply(&change(2, "a", 2, "A2")),
            ApplyResult::Applied
        );
        assert_eq!(
            replica.apply(&change(1, "a", 1, "A1")),
            ApplyResult::Duplicate
        );
        assert_eq!(
            replica.apply(&change(2, "a", 2, "A2")),
            ApplyResult::Duplicate
        );
        assert_eq!(replica.checkpoint(), 2);
        assert_eq!(replica.records()["a"].title, "A2");
        assert_eq!(replica.records()["a"].revision, 2);
        assert_eq!(replica.applied_sequences(), [1, 2]);
    }

    #[test]
    fn a_gap_applies_nothing() {
        let mut replica = Replica::new();
        replica.apply(&change(1, "a", 1, "A1"));
        assert_eq!(
            replica.apply(&change(3, "b", 1, "B1")),
            ApplyResult::Gap {
                checkpoint: 1,
                received: 3
            }
        );
        assert_eq!(replica.checkpoint(), 1);
        assert!(!replica.records().contains_key("b"));
    }
}
