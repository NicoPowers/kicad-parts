//! A local copy of every part's current revision and current details, kept
//! in step with the team server's Changes.
//!
//! Changes are numbered by `seq` without gaps. The replica applies them
//! strictly in that order and remembers the last one it applied (its
//! checkpoint). A Change can arrive twice, through a realtime notification
//! and through catching up after a disconnect; the second copy is ignored,
//! so nothing is missed or applied twice. This follows
//! `collaboration::Replica`, which does the same for the provisional
//! `collab_*` records.

use crate::part::{PartDetails, PartRevision};
use crate::pocketbase::{
    StoredArchiveEntry, StoredChange, StoredPartDetails, StoredPartRevision,
    archive_entries_in_change, changes_since, details_in_change, part_revisions_by_id,
    revisions_in_change,
};
use collaboration::{ApplyResult, CollabError, CollabResult, PocketBaseClient};
use std::collections::BTreeMap;

/// What one Change saved, as the replica needs it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangeRecords {
    pub details: Vec<StoredPartDetails>,
    pub revisions: Vec<StoredPartRevision>,
    pub archive_entries: Vec<StoredArchiveEntry>,
    /// The revisions the archive entries make current, which an earlier
    /// Change saved.
    pub made_current: Vec<StoredPartRevision>,
}

impl ChangeRecords {
    /// Everything the Change with ID `change_id` saved.
    pub fn fetch(client: &PocketBaseClient, change_id: &str) -> CollabResult<Self> {
        let archive_entries = archive_entries_in_change(client, change_id)?;
        let ids: Vec<&str> = archive_entries
            .iter()
            .map(|entry| entry.current_revision_after.as_str())
            .collect();
        let made_current = if ids.is_empty() {
            Vec::new()
        } else {
            part_revisions_by_id(client, &ids)?
        };
        Ok(Self {
            details: details_in_change(client, change_id)?,
            revisions: revisions_in_change(client, change_id)?,
            archive_entries,
            made_current,
        })
    }
}

/// A part's current revision and current details as the replica knows
/// them. A part appears once the replica has seen both (every new part
/// gets details 1 and revision 1 in the same Change).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReplicaPart {
    pub revision_id: String,
    /// The current revision's number.
    pub number: u32,
    pub snapshot_sha256: String,
    /// The current revision, when its snapshot parses.
    pub revision: Option<PartRevision>,
    pub details_id: String,
    pub details_number: u32,
    /// The current part details, when their snapshot parses.
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

    /// Current revisions and details by part ID.
    pub fn parts(&self) -> &BTreeMap<String, ReplicaPart> {
        &self.parts
    }

    /// The current revision and details of the part with `ipn`, if known.
    pub fn part_by_ipn(&self, ipn: &str) -> Option<&ReplicaPart> {
        self.parts.get(self.ipns.get(ipn)?)
    }

    /// Every applied `seq`, in the order applied.
    pub fn applied_sequences(&self) -> &[u64] {
        &self.applied
    }

    /// Applies `change`, which saved `records`, if it is the next one after
    /// the checkpoint. A Change may hold only details, only revisions, only
    /// an archive entry, or a mix.
    pub fn apply(&mut self, change: &StoredChange, records: &ChangeRecords) -> ApplyResult {
        if change.seq <= self.checkpoint {
            return ApplyResult::Duplicate;
        }
        if change.seq != self.checkpoint + 1 {
            return ApplyResult::Gap {
                checkpoint: self.checkpoint,
                received: change.seq,
            };
        }
        for saved in records
            .details
            .iter()
            .filter(|saved| saved.change == change.id)
        {
            let part = self.parts.entry(saved.part.clone()).or_default();
            if part.details_number < saved.number {
                let parsed: Option<PartDetails> = serde_json::from_str(&saved.snapshot).ok();
                if let Some(parsed) = &parsed {
                    self.ipns.insert(parsed.ipn.clone(), saved.part.clone());
                }
                part.details_id.clone_from(&saved.id);
                part.details_number = saved.number;
                part.details = parsed;
            }
        }
        // A new revision is always the part's newest, so it becomes current.
        for revision in records
            .revisions
            .iter()
            .filter(|revision| revision.change == change.id)
        {
            self.make_current(revision);
        }
        // An archive or unarchive moves the current revision to the one its
        // entry names, which may be older.
        for entry in records
            .archive_entries
            .iter()
            .filter(|entry| entry.change == change.id)
        {
            match records
                .made_current
                .iter()
                .find(|revision| revision.id == entry.current_revision_after)
            {
                Some(revision) => self.make_current(revision),
                None => {
                    let part = self.parts.entry(entry.part.clone()).or_default();
                    part.revision_id.clone_from(&entry.current_revision_after);
                    part.number = entry.current_revision_number_after;
                    part.snapshot_sha256.clear();
                    part.revision = None;
                }
            }
        }
        self.checkpoint = change.seq;
        self.applied.push(change.seq);
        ApplyResult::Applied
    }

    fn make_current(&mut self, revision: &StoredPartRevision) {
        let parsed: Option<PartRevision> = serde_json::from_str(&revision.snapshot).ok();
        if let Some(parsed) = &parsed {
            self.ipns.insert(parsed.ipn.clone(), revision.part.clone());
        }
        let part = self.parts.entry(revision.part.clone()).or_default();
        part.revision_id.clone_from(&revision.id);
        part.number = revision.number;
        part.snapshot_sha256.clone_from(&revision.snapshot_sha256);
        part.revision = parsed;
    }

    /// Fetches and applies every Change after the checkpoint. Returns how
    /// many were applied.
    pub fn catch_up(&mut self, client: &PocketBaseClient) -> CollabResult<usize> {
        let mut applied = 0;
        for change in changes_since(client, self.checkpoint)? {
            let records = ChangeRecords::fetch(client, &change.id)?;
            match self.apply(&change, &records) {
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
        let records = ChangeRecords::fetch(client, &change.id)?;
        Ok(self.apply(change, &records))
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

    fn details(seq: u64, part: &str, number: u32) -> StoredPartDetails {
        StoredPartDetails {
            id: format!("{part}-details-{number}"),
            part: part.into(),
            number,
            predecessor: String::new(),
            change: format!("change{seq}"),
            snapshot: "{}".into(),
            snapshot_sha256: format!("{number:064}"),
            source: "{}".into(),
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

    fn records(
        details: &[StoredPartDetails],
        revisions: &[StoredPartRevision],
        archive_entries: &[StoredArchiveEntry],
        made_current: &[StoredPartRevision],
    ) -> ChangeRecords {
        ChangeRecords {
            details: details.to_vec(),
            revisions: revisions.to_vec(),
            archive_entries: archive_entries.to_vec(),
            made_current: made_current.to_vec(),
        }
    }

    fn archive(
        seq: u64,
        part: &str,
        number: u32,
        archived: bool,
        current: u32,
    ) -> StoredArchiveEntry {
        StoredArchiveEntry {
            id: format!("archive{seq}"),
            part: part.into(),
            part_revision: format!("{part}-{number}"),
            revision_number: number,
            archived,
            change: format!("change{seq}"),
            seq,
            current_revision_after: format!("{part}-{current}"),
            current_revision_number_after: current,
        }
    }

    #[test]
    fn changes_apply_in_order_once() {
        let mut replica = CatalogReplica::new();
        assert_eq!(
            replica.apply(
                &change(1),
                &records(
                    &[details(1, "p", 1), details(1, "q", 1)],
                    &[revision(1, "p", 1), revision(1, "q", 1)],
                    &[],
                    &[]
                )
            ),
            ApplyResult::Applied
        );
        assert_eq!(
            replica.apply(&change(3), &records(&[], &[revision(3, "p", 3)], &[], &[])),
            ApplyResult::Gap {
                checkpoint: 1,
                received: 3
            }
        );
        assert_eq!(
            replica.apply(&change(2), &records(&[], &[revision(2, "p", 2)], &[], &[])),
            ApplyResult::Applied
        );
        assert_eq!(
            replica.apply(&change(2), &records(&[], &[revision(2, "p", 2)], &[], &[])),
            ApplyResult::Duplicate
        );
        // A Change that saves only details moves the details, not the
        // revision.
        assert_eq!(
            replica.apply(&change(3), &records(&[details(3, "p", 2)], &[], &[], &[])),
            ApplyResult::Applied
        );
        assert_eq!(replica.checkpoint(), 3);
        assert_eq!(
            (
                replica.parts()["p"].number,
                replica.parts()["p"].details_number
            ),
            (2, 2)
        );
        assert_eq!(
            (
                replica.parts()["q"].number,
                replica.parts()["q"].details_number
            ),
            (1, 1)
        );
        // A Change that holds only an archive entry moves the current
        // revision back to the one the entry names; unarchiving moves it
        // forward again.
        assert_eq!(
            replica.apply(
                &change(4),
                &records(
                    &[],
                    &[],
                    &[archive(4, "p", 2, true, 1)],
                    &[revision(1, "p", 1)]
                )
            ),
            ApplyResult::Applied
        );
        assert_eq!(
            (
                replica.parts()["p"].number,
                replica.parts()["p"].revision_id.as_str()
            ),
            (1, "p-1")
        );
        assert_eq!(
            replica.apply(
                &change(5),
                &records(
                    &[],
                    &[],
                    &[archive(5, "p", 2, false, 2)],
                    &[revision(2, "p", 2)]
                )
            ),
            ApplyResult::Applied
        );
        assert_eq!(replica.parts()["p"].number, 2);
        assert_eq!(replica.applied_sequences(), [1, 2, 3, 4, 5]);
    }
}
