//! A local copy of every part's current revision and current details, kept
//! in step with the team server's Changes.
//!
//! # Following Changes
//!
//! Changes are numbered by `seq` without gaps. The replica applies them
//! strictly in that order and remembers the last one it applied (its
//! checkpoint). A Change can arrive twice, through a realtime notification
//! and through catching up after a disconnect; the second copy is ignored,
//! so nothing is missed or applied twice. This follows
//! `collaboration::Replica`, which does the same for the provisional
//! `collab_*` records.
//!
//! # Starting at sign-in
//!
//! Replaying every Change from the first would download every revision and
//! every saved copy of part details ever made, so sign-in would get slower
//! as the history grows. [`CatalogReplica::from_server`] reads the current
//! state instead, then replays only the Changes saved meanwhile:
//!
//! 1. Note the newest Change's `seq`, N.
//! 2. Read every part's current revision and current details, in pages
//!    ordered by part ID.
//! 3. Take N as the checkpoint, and replay every Change after N in order.
//!
//! Others may save while the pages are read. So each part is read as it
//! was at some moment after Change N, not all at the same moment, and a
//! part added meanwhile may be missed. The replay still ends exactly right:
//!
//! - Only Changes move a part's current revision or details. A Change saves
//!   everything it moves in one transaction, and the server's write guards
//!   (`catalog_write_guards.pb.js`) refuse any other move. Step 3 lists the
//!   Changes after step 2 ends, so it includes every Change whose work step
//!   2 saw. If one were missing, [`CatalogReplica::apply`] would refuse the
//!   gap and the sign-in would fail and be tried again.
//! - Applying a Change sets what it saved, whatever the replica held: its
//!   new revision, or the revision its archive entry names, becomes
//!   current, and so do its new details. So a part that a replayed Change
//!   touched ends as the last such Change left it, as on the server.
//! - A part that no Change after N touched has not moved since N, so its
//!   page read it right.
//! - Parts are never deleted, and paging by ID never skips a part that
//!   existed before step 2 began. So a part the pages missed was added by a
//!   Change after N, and the replay adds it.
//!
//! During the replay, a part may briefly go back to an older state. Nobody
//! sees that, because `from_server` returns only the finished replica.

use crate::change::ChangeKind;
use crate::part::{PartDetails, PartRevision};
use crate::pocketbase::{
    StoredArchiveEntry, StoredChange, StoredCurrentPart, StoredCurrentRecord, StoredPartDetails,
    StoredPartRevision, archive_entries_in_changes, changes_since, current_parts_after,
    details_in_changes, latest_change_seq, part_revisions_by_id, revisions_in_changes,
};
use collaboration::{ApplyResult, CollabError, CollabResult, PocketBaseClient};
use std::collections::BTreeMap;

/// Parts read per request at sign-in. A part with its current revision and
/// details is about 1.5 KB, so a full page is under 1 MB.
pub const PARTS_PER_PAGE: usize = 500;

/// The most records PocketBase returns in one page; it quietly sends this
/// many when asked for more.
const POCKETBASE_MAX_PAGE: usize = 1000;

/// Changes whose records one catch-up step reads together: a few requests
/// for all of them, instead of four for each.
const CHANGES_PER_READ: usize = 40;

/// What some Changes saved, as the replica needs it. Each record names its
/// Change, so one read can serve several Changes.
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
    /// Everything `changes` saved.
    pub fn fetch(client: &PocketBaseClient, changes: &[StoredChange]) -> CollabResult<Self> {
        let change_ids: Vec<&str> = changes.iter().map(|change| change.id.as_str()).collect();
        // The server saves archive entries only in archive and unarchive
        // Changes (catalog_write_guards.pb.js), so only those are asked
        // about. Asking about the others would find nothing, and PocketBase
        // slows down replies that find nothing, by up to half a second,
        // once there are more than three in three seconds. A kind this
        // version does not know is asked about too.
        let archiving: Vec<&str> = changes
            .iter()
            .filter(|change| {
                matches!(
                    ChangeKind::parse(&change.kind),
                    Some(ChangeKind::Archive | ChangeKind::Unarchive) | None
                )
            })
            .map(|change| change.id.as_str())
            .collect();
        let archive_entries = archive_entries_in_changes(client, &archiving)?;
        let ids: Vec<&str> = archive_entries
            .iter()
            .map(|entry| entry.current_revision_after.as_str())
            .collect();
        let made_current = part_revisions_by_id(client, &ids)?;
        // Without the revision an entry makes current, the replica cannot
        // apply the Change; an error makes the follower reconnect and catch
        // up instead of guessing.
        if let Some(missing) = archive_entries.iter().find(|entry| {
            !made_current
                .iter()
                .any(|revision| revision.id == entry.current_revision_after)
        }) {
            return Err(CollabError::Protocol(format!(
                "Change {} makes revision {} current, which could not be fetched",
                missing.change, missing.current_revision_after
            )));
        }
        Ok(Self {
            details: details_in_changes(client, &change_ids)?,
            revisions: revisions_in_changes(client, &change_ids)?,
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

/// One saved part revision, as the replica takes it, wherever it was read
/// from. Revisions and details have their own types, so neither can be
/// taken as the other.
struct SavedRevision<'a> {
    part: &'a str,
    id: &'a str,
    number: u32,
    snapshot: &'a str,
    snapshot_sha256: &'a str,
}

/// One saved copy of part details, as the replica takes it, wherever it was
/// read from.
struct SavedDetails<'a> {
    part: &'a str,
    id: &'a str,
    number: u32,
    snapshot: &'a str,
}

impl<'a> SavedRevision<'a> {
    fn current(part: &'a str, record: &'a StoredCurrentRecord) -> Self {
        Self {
            part,
            id: &record.id,
            number: record.number,
            snapshot: &record.snapshot,
            snapshot_sha256: &record.snapshot_sha256,
        }
    }
}

impl<'a> SavedDetails<'a> {
    fn current(part: &'a str, record: &'a StoredCurrentRecord) -> Self {
        Self {
            part,
            id: &record.id,
            number: record.number,
            snapshot: &record.snapshot,
        }
    }
}

impl<'a> From<&'a StoredPartRevision> for SavedRevision<'a> {
    fn from(revision: &'a StoredPartRevision) -> Self {
        Self {
            part: &revision.part,
            id: &revision.id,
            number: revision.number,
            snapshot: &revision.snapshot,
            snapshot_sha256: &revision.snapshot_sha256,
        }
    }
}

impl<'a> From<&'a StoredPartDetails> for SavedDetails<'a> {
    fn from(details: &'a StoredPartDetails) -> Self {
        Self {
            part: &details.part,
            id: &details.id,
            number: details.number,
            snapshot: &details.snapshot,
        }
    }
}

impl CatalogReplica {
    pub fn new() -> Self {
        Self::default()
    }

    /// A replica of the team server as it is now, for signing in. It reads
    /// every part's current state in pages, then replays the Changes saved
    /// meanwhile; the module documentation explains why that ends right.
    pub fn from_server(client: &PocketBaseClient) -> CollabResult<Self> {
        Self::from_server_in_pages(client, PARTS_PER_PAGE)
    }

    /// [`CatalogReplica::from_server`], reading `parts_per_page` parts per
    /// request. Tests use small pages so that others can save between them.
    pub fn from_server_in_pages(
        client: &PocketBaseClient,
        parts_per_page: usize,
    ) -> CollabResult<Self> {
        // A page shorter than asked for marks the end, so never ask for
        // more than PocketBase sends.
        let parts_per_page = parts_per_page.clamp(1, POCKETBASE_MAX_PAGE);
        let mut replica = Self {
            checkpoint: latest_change_seq(client)?,
            ..Self::default()
        };
        let mut after = String::new();
        loop {
            let page = current_parts_after(client, &after, parts_per_page)?;
            for part in &page {
                replica.read_part(part);
            }
            match page.last() {
                Some(last) if page.len() == parts_per_page => after.clone_from(&last.id),
                _ => break,
            }
        }
        replica.catch_up(client)?;
        Ok(replica)
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

    /// Every `seq` applied one by one, in the order applied. Changes that a
    /// replica from [`CatalogReplica::from_server`] read as part of the
    /// current state are not listed.
    pub fn applied_sequences(&self) -> &[u64] {
        &self.applied
    }

    /// Applies `change`, which saved `records`, if it is the next one after
    /// the checkpoint. A Change may hold only details, only revisions, only
    /// an archive entry, or a mix. `records` may also hold what other
    /// Changes saved, which is ignored.
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
        let archive_entries: Vec<&StoredArchiveEntry> = records
            .archive_entries
            .iter()
            .filter(|entry| entry.change == change.id)
            .collect();
        // Every revision an archive entry makes current must be at hand;
        // otherwise nothing is applied and the caller catches up.
        let made_current: Option<Vec<&StoredPartRevision>> = archive_entries
            .iter()
            .map(|entry| {
                records
                    .made_current
                    .iter()
                    .find(|revision| revision.id == entry.current_revision_after)
            })
            .collect();
        let Some(made_current) = made_current else {
            return ApplyResult::Gap {
                checkpoint: self.checkpoint,
                received: change.seq,
            };
        };
        for saved in records
            .details
            .iter()
            .filter(|saved| saved.change == change.id)
        {
            self.set_current_details(saved.into());
        }
        // A new revision is always the part's newest, so it becomes current.
        for revision in records
            .revisions
            .iter()
            .filter(|revision| revision.change == change.id)
        {
            self.set_current_revision(revision.into());
        }
        // An archive or unarchive moves the current revision to the one its
        // entry names, which may be older.
        for revision in made_current {
            self.set_current_revision(revision.into());
        }
        self.checkpoint = change.seq;
        self.applied.push(change.seq);
        ApplyResult::Applied
    }

    /// Takes a part as one page of the sign-in read found it.
    fn read_part(&mut self, part: &StoredCurrentPart) {
        self.set_current_details(SavedDetails::current(&part.id, &part.details));
        self.set_current_revision(SavedRevision::current(&part.id, &part.revision));
    }

    /// Makes `saved` the part's current details. Like
    /// [`CatalogReplica::set_current_revision`], it takes what it is given, even if
    /// the replica held newer details: that only happens while replaying
    /// over a sign-in read, and the replay then reaches the newer ones too.
    fn set_current_details(&mut self, saved: SavedDetails) {
        let parsed: Option<PartDetails> = serde_json::from_str(saved.snapshot).ok();
        if let Some(parsed) = &parsed {
            self.ipns.insert(parsed.ipn.clone(), saved.part.to_owned());
        }
        let part = self.parts.entry(saved.part.to_owned()).or_default();
        part.details_id = saved.id.to_owned();
        part.details_number = saved.number;
        part.details = parsed;
    }

    /// Makes `saved` the part's current revision, even if it is older:
    /// archiving moves the current revision back.
    fn set_current_revision(&mut self, saved: SavedRevision) {
        let parsed: Option<PartRevision> = serde_json::from_str(saved.snapshot).ok();
        if let Some(parsed) = &parsed {
            self.ipns.insert(parsed.ipn.clone(), saved.part.to_owned());
        }
        let part = self.parts.entry(saved.part.to_owned()).or_default();
        part.revision_id = saved.id.to_owned();
        part.number = saved.number;
        part.snapshot_sha256 = saved.snapshot_sha256.to_owned();
        part.revision = parsed;
    }

    /// Fetches and applies every Change after the checkpoint. Returns how
    /// many were applied.
    pub fn catch_up(&mut self, client: &PocketBaseClient) -> CollabResult<usize> {
        let changes = changes_since(client, self.checkpoint)?;
        let mut applied = 0;
        for batch in changes.chunks(CHANGES_PER_READ) {
            let records = ChangeRecords::fetch(client, batch)?;
            for change in batch {
                match self.apply(change, &records) {
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
        let records = ChangeRecords::fetch(client, std::slice::from_ref(change))?;
        Ok(self.apply(change, &records))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::part::{PART_DETAILS_SCHEMA, PART_REVISION_SCHEMA};

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

    /// The IPN of test part `part`, such as `P` for `p`.
    fn ipn(part: &str) -> String {
        part.to_uppercase()
    }

    /// Details `number` of `part`, with its number as the Value, so every
    /// saved copy differs.
    fn details_snapshot(part: &str, number: u32) -> String {
        PartDetails {
            schema: PART_DETAILS_SCHEMA.into(),
            ipn: ipn(part),
            category: "res".into(),
            fields: vec![crate::part::DetailField {
                name: "Value".into(),
                value: number.to_string(),
            }],
            manufacturer_parts: vec![],
        }
        .canonical_json()
    }

    fn revision_snapshot(part: &str, number: u32) -> String {
        PartRevision {
            schema: PART_REVISION_SCHEMA.into(),
            ipn: ipn(part),
            cad_fields: vec![crate::part::DetailField {
                name: "Symbol".into(),
                value: format!("lib:{part}-{number}"),
            }],
            cad_references: vec![],
            excluded_from_bom: false,
        }
        .canonical_json()
    }

    fn details(seq: u64, part: &str, number: u32) -> StoredPartDetails {
        StoredPartDetails {
            id: format!("{part}-details-{number}"),
            part: part.into(),
            number,
            predecessor: String::new(),
            change: format!("change{seq}"),
            snapshot: details_snapshot(part, number),
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
            snapshot: revision_snapshot(part, number),
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
        // An archive entry whose revision is not at hand applies nothing.
        assert_eq!(
            replica.apply(
                &change(6),
                &records(&[], &[], &[archive(6, "p", 2, true, 1)], &[])
            ),
            ApplyResult::Gap {
                checkpoint: 5,
                received: 6
            }
        );
        assert_eq!(replica.parts()["p"].number, 2);
        assert_eq!(replica.applied_sequences(), [1, 2, 3, 4, 5]);
    }

    /// A short team history, Change by Change, that moves each kind of
    /// state: new parts, new details, new revisions, and archives and an
    /// unarchive, which move the current revision back and forth. It ends
    /// with an archive, so a part ends on an older revision than it had.
    fn history() -> Vec<(StoredChange, ChangeRecords)> {
        vec![
            (
                change(1),
                records(
                    &[details(1, "p", 1), details(1, "q", 1)],
                    &[revision(1, "p", 1), revision(1, "q", 1)],
                    &[],
                    &[],
                ),
            ),
            (change(2), records(&[details(2, "p", 2)], &[], &[], &[])),
            (
                change(3),
                records(
                    &[details(3, "r", 1)],
                    &[revision(3, "p", 2), revision(3, "r", 1)],
                    &[],
                    &[],
                ),
            ),
            (
                change(4),
                records(
                    &[],
                    &[],
                    &[archive(4, "p", 2, true, 1)],
                    &[revision(1, "p", 1)],
                ),
            ),
            (
                change(5),
                records(&[details(5, "q", 2)], &[revision(5, "q", 2)], &[], &[]),
            ),
            (
                change(6),
                records(
                    &[],
                    &[],
                    &[archive(6, "p", 2, false, 2)],
                    &[revision(2, "p", 2)],
                ),
            ),
            (
                change(7),
                records(
                    &[],
                    &[],
                    &[archive(7, "p", 2, true, 1)],
                    &[revision(1, "p", 1)],
                ),
            ),
        ]
    }

    /// `part` as a sign-in page reads it from a replica in that state.
    fn page_entry(part: &str, known: &ReplicaPart) -> StoredCurrentPart {
        StoredCurrentPart {
            id: part.into(),
            revision: StoredCurrentRecord {
                id: known.revision_id.clone(),
                number: known.number,
                snapshot: revision_snapshot(part, known.number),
                snapshot_sha256: known.snapshot_sha256.clone(),
            },
            details: StoredCurrentRecord {
                id: known.details_id.clone(),
                number: known.details_number,
                snapshot: details_snapshot(part, known.details_number),
                snapshot_sha256: format!("{:064}", known.details_number),
            },
        }
    }

    /// Every way to pick, for each part, the moment its page was read: any
    /// state from `noted` to the last, or never (missed by paging). A part
    /// that existed at `noted` is never missed.
    fn read_moments(
        noted: usize,
        last: usize,
        parts: &[&str],
        at: &[CatalogReplica],
    ) -> Vec<Vec<Option<usize>>> {
        let mut ways = vec![vec![]];
        for part in parts {
            let mut choices: Vec<Option<usize>> = (noted..=last).map(Some).collect();
            if !at[noted].parts().contains_key(*part) {
                choices.push(None);
            }
            ways = ways
                .into_iter()
                .flat_map(|way| {
                    choices.iter().map(move |choice| {
                        let mut way = way.clone();
                        way.push(*choice);
                        way
                    })
                })
                .collect();
        }
        ways
    }

    #[test]
    fn a_sign_in_read_ends_right_whenever_others_save_during_it() {
        // The replica after each Change, applied one by one: at[k] has
        // Changes 1 to k.
        let history = history();
        let mut at = vec![CatalogReplica::new()];
        for (change, records) in &history {
            let mut next = at.last().unwrap().clone();
            assert_eq!(next.apply(change, records), ApplyResult::Applied);
            at.push(next);
        }
        let last = history.len();
        // What the server holds at the end: revision and details numbers.
        let numbers = |replica: &CatalogReplica| -> Vec<(String, u32, u32)> {
            replica
                .parts()
                .iter()
                .map(|(id, part)| (id.clone(), part.number, part.details_number))
                .collect()
        };
        assert_eq!(
            numbers(&at[last]),
            [("p".into(), 1, 2), ("q".into(), 2, 2), ("r".into(), 1, 1)]
        );
        let parts = ["p", "q", "r"];
        let mut cases = 0;
        // Sign-in notes Change `noted` as the newest, then reads each part
        // at its own moment while others save Changes up to the last.
        for noted in 0..=last {
            for moments in read_moments(noted, last, &parts, &at) {
                let mut replica = CatalogReplica {
                    checkpoint: noted as u64,
                    ..CatalogReplica::default()
                };
                for (part, moment) in parts.iter().zip(&moments) {
                    if let Some(known) = moment.and_then(|k| at[k].parts().get(*part)) {
                        replica.read_part(&page_entry(part, known));
                    }
                }
                for (change, records) in &history[noted..] {
                    assert_eq!(replica.apply(change, records), ApplyResult::Applied);
                }
                assert_eq!(
                    (replica.checkpoint(), replica.parts(), &replica.ipns),
                    (at[last].checkpoint(), at[last].parts(), &at[last].ipns),
                    "noted {noted}, parts read at {moments:?}"
                );
                cases += 1;
            }
        }
        assert!(cases > 500, "only {cases} cases");
    }

    #[test]
    fn a_change_that_arrives_twice_applies_once() {
        // Sign-in noted Change 3, read the parts after Change 4 had been
        // saved, then replayed Change 4.
        let history = history();
        let mut at4 = CatalogReplica::new();
        for (change, records) in &history[..4] {
            assert_eq!(at4.apply(change, records), ApplyResult::Applied);
        }
        let mut replica = CatalogReplica {
            checkpoint: 3,
            ..CatalogReplica::default()
        };
        for (part, known) in at4.parts() {
            replica.read_part(&page_entry(part, known));
        }
        let (fourth, records) = &history[3];
        assert_eq!(replica.apply(fourth, records), ApplyResult::Applied);
        let signed_in = replica.clone();
        // The realtime notification of Change 4, sent while signing in,
        // comes in afterwards: it changes nothing. So does an older one.
        assert_eq!(replica.apply(fourth, records), ApplyResult::Duplicate);
        assert_eq!(
            replica.apply(&history[1].0, &history[1].1),
            ApplyResult::Duplicate
        );
        assert_eq!(replica, signed_in);
        // The next Change follows on.
        let (fifth, records) = &history[4];
        assert_eq!(replica.apply(fifth, records), ApplyResult::Applied);
        assert_eq!(replica.parts()["q"].number, 2);
        assert_eq!(replica.applied_sequences(), [4, 5]);
    }

    #[test]
    fn records_of_other_changes_are_ignored() {
        // One catch-up reads the records of many Changes at once; each
        // Change applies only its own.
        let all = history()
            .into_iter()
            .fold(ChangeRecords::default(), |mut all, (_, records)| {
                all.details.extend(records.details);
                all.revisions.extend(records.revisions);
                all.archive_entries.extend(records.archive_entries);
                all.made_current.extend(records.made_current);
                all
            });
        let mut together = CatalogReplica::new();
        let mut one_by_one = CatalogReplica::new();
        for (change, records) in history() {
            assert_eq!(together.apply(&change, &all), ApplyResult::Applied);
            assert_eq!(one_by_one.apply(&change, &records), ApplyResult::Applied);
            assert_eq!(together, one_by_one, "after Change {}", change.seq);
        }
        assert_eq!(together.part_by_ipn("P").unwrap().number, 1);
    }
}
