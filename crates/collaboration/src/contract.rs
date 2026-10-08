//! The wire contract of `POST /api/collab/commands` and of the provisional
//! `collab_*` collections.

use crate::{CollabError, CollabResult};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Names one command attempt. A retry of the same command reuses it, and
/// the server answers a repeat with the stored result instead of applying
/// it twice.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CorrelationId(String);

impl CorrelationId {
    /// A new random ID, `cmd-` and 26 letters and digits.
    pub fn generate() -> Self {
        const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut bytes = [0u8; 26];
        getrandom::fill(&mut bytes).expect("the OS random source is available");
        // The slight modulo bias is harmless: IDs need to be unique, not
        // uniform, and 26 characters leave about 134 bits.
        let suffix: String = bytes
            .iter()
            .map(|byte| ALPHABET[(*byte % 36) as usize] as char)
            .collect();
        Self(format!("cmd-{suffix}"))
    }

    /// Accepts 1 to 100 letters, digits, `_`, `.`, `:` or `-`, the same
    /// rule the server enforces.
    pub fn new(value: impl Into<String>) -> CollabResult<Self> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 100
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte));
        if valid {
            Ok(Self(value))
        } else {
            Err(CollabError::Protocol(format!(
                "`{value}` is not a valid correlation ID"
            )))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CorrelationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Sets a record's title and content if it is still at
/// `expected_revision` (0: the record must not exist yet).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditRecord {
    pub correlation_id: CorrelationId,
    pub record_key: String,
    pub expected_revision: u64,
    pub title: String,
    pub content: String,
}

impl EditRecord {
    /// An edit with a fresh correlation ID.
    pub fn new(
        record_key: impl Into<String>,
        expected_revision: u64,
        title: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            correlation_id: CorrelationId::generate(),
            record_key: record_key.into(),
            expected_revision,
            title: title.into(),
            content: content.into(),
        }
    }
}

/// What the server decided.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CommandOutcome {
    /// 200: written as `revision`, logged as change `seq`.
    Applied {
        correlation_id: CorrelationId,
        record_key: String,
        record_id: String,
        revision: u64,
        seq: u64,
    },
    /// 422: rejected because it would change nothing.
    Noop {
        correlation_id: CorrelationId,
        record_key: String,
        record_id: String,
        revision: u64,
    },
    /// 409: the record is no longer at the expected revision.
    Conflict {
        correlation_id: CorrelationId,
        record_key: String,
        expected_revision: u64,
        current_revision: u64,
    },
    /// 409: the correlation ID already names a different command.
    CorrelationReused { correlation_id: CorrelationId },
}

/// The server's answer to a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandReply {
    pub status: u16,
    pub outcome: CommandOutcome,
    /// True when the server returned a stored result for a repeated
    /// correlation ID instead of evaluating the command again.
    pub replayed: bool,
    /// The exact response body, so callers can check replays are
    /// byte-identical.
    pub body: String,
}

/// One entry of the append-only `collab_changes` log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEntry {
    pub id: String,
    /// Gap-free and increasing across the whole log: the reconciliation
    /// checkpoint.
    pub seq: u64,
    pub record: String,
    pub record_key: String,
    pub revision: u64,
    pub correlation_id: String,
    pub actor: String,
    pub title: String,
    pub content: String,
}

/// The current state of a `collab_records` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordSnapshot {
    pub id: String,
    pub key: String,
    pub title: String,
    pub content: String,
    pub revision: u64,
    pub updated_by: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correlation_ids_follow_the_server_rule() {
        let generated = CorrelationId::generate();
        assert!(generated.as_str().starts_with("cmd-"));
        assert_eq!(generated.as_str().len(), 30);
        assert!(CorrelationId::new(generated.as_str()).is_ok());
        assert_ne!(generated, CorrelationId::generate());
        assert!(CorrelationId::new("").is_err());
        assert!(CorrelationId::new("has space").is_err());
        assert!(CorrelationId::new("x".repeat(101)).is_err());
        assert!(CorrelationId::new("a_b.c:d-1").is_ok());
    }

    #[test]
    fn commands_serialize_in_the_route_format() {
        let command = EditRecord {
            correlation_id: CorrelationId::new("c-1").unwrap(),
            record_key: "res-1".into(),
            expected_revision: 3,
            title: "T".into(),
            content: "C".into(),
        };
        assert_eq!(
            serde_json::to_string(&command).unwrap(),
            r#"{"correlationId":"c-1","recordKey":"res-1","expectedRevision":3,"title":"T","content":"C"}"#
        );
    }

    #[test]
    fn every_outcome_parses() {
        let applied: CommandOutcome = serde_json::from_str(
            r#"{"correlationId":"c","outcome":"applied","recordId":"r","recordKey":"k","revision":2,"seq":7}"#,
        )
        .unwrap();
        assert_eq!(
            applied,
            CommandOutcome::Applied {
                correlation_id: CorrelationId::new("c").unwrap(),
                record_key: "k".into(),
                record_id: "r".into(),
                revision: 2,
                seq: 7,
            }
        );
        let conflict: CommandOutcome = serde_json::from_str(
            r#"{"correlationId":"c","currentRevision":2,"expectedRevision":1,"message":"m","outcome":"conflict","recordKey":"k"}"#,
        )
        .unwrap();
        assert!(matches!(
            conflict,
            CommandOutcome::Conflict {
                current_revision: 2,
                ..
            }
        ));
        let noop: CommandOutcome = serde_json::from_str(
            r#"{"correlationId":"c","outcome":"noop","recordId":"r","recordKey":"k","revision":2}"#,
        )
        .unwrap();
        assert!(matches!(noop, CommandOutcome::Noop { revision: 2, .. }));
        let reused: CommandOutcome = serde_json::from_str(
            r#"{"correlationId":"c","message":"m","outcome":"correlation_reused"}"#,
        )
        .unwrap();
        assert!(matches!(reused, CommandOutcome::CorrelationReused { .. }));
        assert!(serde_json::from_str::<CommandOutcome>(r#"{"outcome":"other"}"#).is_err());
    }
}
