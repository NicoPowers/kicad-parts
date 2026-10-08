//! The Catalog's PocketBase contract: the protected import route
//! (`infra/pocketbase/pb_hooks/catalog_imports.pb.js`) and the `catalog_*`
//! collections (`infra/pocketbase/pb_migrations/1791500000_catalog_init.js`).

use crate::legacy_import::LegacyImport;
use crate::sha256_hex;
use collaboration::{CollabError, CollabResult, PocketBaseClient};
use serde::Deserialize;
use serde_json::{Value, json};

/// The route that publishes a Legacy Catalog import.
pub const LEGACY_IMPORT_ROUTE: &str = "/api/catalog/legacy-imports";

/// Every Catalog collection, in dependency order.
pub const COLLECTIONS: [&str; 9] = [
    "catalog_change_sets",
    "catalog_asset_blobs",
    "catalog_assets",
    "catalog_asset_revisions",
    "catalog_managed_parts",
    "catalog_part_revisions",
    "catalog_import_diagnostics",
    "catalog_audit_events",
    "catalog_commands",
];

/// One import request: what to publish, why, and its idempotency key.
#[derive(Clone, Debug)]
pub struct ImportCommand<'a> {
    pub correlation_id: String,
    pub reason: String,
    pub import: &'a LegacyImport,
}

impl<'a> ImportCommand<'a> {
    /// A command whose correlation ID is derived from the import itself, so
    /// retrying the same import replays the stored answer.
    pub fn new(import: &'a LegacyImport, reason: &str) -> Self {
        Self {
            correlation_id: format!("legacy-import-{}", &import.fingerprint()[..32]),
            reason: reason.to_owned(),
            import,
        }
    }

    pub fn body(&self) -> Value {
        let import = self.import;
        json!({
            "correlationId": self.correlation_id,
            "reason": self.reason,
            "legacyGrouping": import.legacy_grouping,
            "sourceCommit": import.source_commit,
            "sources": import.sources.iter().map(|source| json!({
                "role": source.role,
                "path": source.path,
                "byteSha256": source.byte_sha256,
                "size": source.size,
                "headerRaw": source.header_raw,
            })).collect::<Vec<_>>(),
            "blobs": import.blobs.iter().map(|blob| json!({
                "sha256": blob.sha256,
                "size": blob.bytes.len(),
                "base64": base64(&blob.bytes),
            })).collect::<Vec<_>>(),
            "assetRevisions": import.asset_revisions.iter().map(|revision| json!({
                "id": revision.id,
                "assetId": revision.asset.id,
                "kind": revision.asset.kind.as_str(),
                "key": revision.asset.key,
                "revisionSha256": revision.revision_sha256,
                "declaration": revision.declaration_json,
                "provenance": revision.provenance_json,
            })).collect::<Vec<_>>(),
            "parts": import.parts.iter().map(|part| json!({
                "id": part.id,
                "ipn": part.ipn.as_str(),
                "snapshot": part.snapshot_json,
                "snapshotSha256": part.snapshot_sha256,
                "provenance": part.provenance_json,
            })).collect::<Vec<_>>(),
            "diagnostics": import.diagnostics.iter().map(|diagnostic| json!({
                "id": diagnostic.id,
                "code": diagnostic.code,
                "subject": diagnostic.subject,
                "path": diagnostic.path,
                "location": diagnostic.location,
                "message": diagnostic.message,
                "detail": diagnostic.detail_json,
            })).collect::<Vec<_>>(),
        })
    }
}

/// What the import route answered.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportReply {
    pub status: u16,
    pub outcome: ImportOutcome,
    /// The answer was replayed from the correlation-ID ledger.
    pub replayed: bool,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ImportOutcome {
    #[serde(rename_all = "camelCase")]
    Applied {
        correlation_id: String,
        change_set_id: String,
        seq: u64,
        created: Counts,
        unchanged: Value,
    },
    #[serde(rename_all = "camelCase")]
    Noop {
        correlation_id: String,
        unchanged: Value,
    },
    #[serde(rename_all = "camelCase")]
    CorrelationReused {
        correlation_id: String,
        message: String,
    },
}

/// What one Change Set created.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub blobs: u64,
    pub assets: u64,
    pub asset_revisions: u64,
    pub managed_parts: u64,
    pub part_revisions: u64,
    pub successor_revisions: u64,
    pub diagnostics: u64,
}

/// Submits `command`. Applied, no-op and correlation-reuse answers are all
/// `Ok`; inspect the outcome.
pub fn submit_import(
    client: &PocketBaseClient,
    command: &ImportCommand<'_>,
) -> CollabResult<ImportReply> {
    let reply = client.post_json(LEGACY_IMPORT_ROUTE, &command.body())?;
    match reply.status {
        200 | 409 | 422 => {
            let outcome: ImportOutcome = serde_json::from_str(&reply.body)
                .map_err(|parse| CollabError::Protocol(format!("{parse}: {}", reply.body)))?;
            Ok(ImportReply {
                status: reply.status,
                outcome,
                replayed: reply.replayed,
                body: reply.body,
            })
        }
        _ => Err(reply.success().err().unwrap_or_else(|| {
            CollabError::Protocol(format!("unexpected status {}", reply.status))
        })),
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredChangeSet {
    pub id: String,
    pub seq: u64,
    pub kind: String,
    pub reason: String,
    pub correlation_id: String,
    pub actor: String,
    pub source_commit: String,
    /// JSON: `{ legacy_grouping, files: [SourceFile] }`.
    pub sources: String,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredManagedPart {
    pub id: String,
    pub ipn: String,
    pub ipn_family: String,
    pub ipn_base: String,
    pub variation: String,
    pub head: String,
    pub head_number: u32,
    pub first_change_set: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredPartRevision {
    pub id: String,
    pub managed_part: String,
    pub number: u32,
    pub predecessor: String,
    pub change_set: String,
    pub snapshot: String,
    pub snapshot_sha256: String,
    pub provenance: String,
    pub asset_revisions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredAsset {
    pub id: String,
    pub kind: String,
    pub key: String,
    pub first_change_set: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredAssetRevision {
    pub id: String,
    pub asset: String,
    pub number: u32,
    pub predecessor: String,
    pub change_set: String,
    pub blob: String,
    pub revision_sha256: String,
    pub declaration: String,
    pub license_status: String,
    pub provenance: String,
}

#[derive(Clone, Deserialize, PartialEq)]
pub struct StoredAssetBlob {
    pub id: String,
    pub sha256: String,
    pub size: u64,
    pub file: String,
    pub first_change_set: String,
    /// The stored file's bytes, downloaded by [`read_back`].
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for StoredAssetBlob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredAssetBlob")
            .field("id", &self.id)
            .field("sha256", &self.sha256)
            .field("size", &self.size)
            .field("downloaded", &self.bytes.len())
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredImportDiagnostic {
    pub id: String,
    pub code: String,
    pub subject: String,
    pub path: String,
    pub location: String,
    pub message: String,
    pub detail: String,
    pub change_set: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredAuditEvent {
    pub id: String,
    pub action: String,
    pub actor: String,
    pub change_set: String,
    pub reason: String,
    pub correlation_id: String,
}

/// Every Catalog record as stored, with each blob's bytes downloaded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CatalogReadback {
    pub change_sets: Vec<StoredChangeSet>,
    pub managed_parts: Vec<StoredManagedPart>,
    pub part_revisions: Vec<StoredPartRevision>,
    pub assets: Vec<StoredAsset>,
    pub asset_revisions: Vec<StoredAssetRevision>,
    pub asset_blobs: Vec<StoredAssetBlob>,
    pub diagnostics: Vec<StoredImportDiagnostic>,
    pub audit_events: Vec<StoredAuditEvent>,
}

impl CatalogReadback {
    pub fn part(&self, ipn: &str) -> Option<&StoredManagedPart> {
        self.managed_parts.iter().find(|part| part.ipn == ipn)
    }

    pub fn part_revision(&self, id: &str) -> Option<&StoredPartRevision> {
        self.part_revisions
            .iter()
            .find(|revision| revision.id == id)
    }

    pub fn asset(&self, id: &str) -> Option<&StoredAsset> {
        self.assets.iter().find(|asset| asset.id == id)
    }

    pub fn asset_revision(&self, id: &str) -> Option<&StoredAssetRevision> {
        self.asset_revisions
            .iter()
            .find(|revision| revision.id == id)
    }

    pub fn blob(&self, id: &str) -> Option<&StoredAssetBlob> {
        self.asset_blobs.iter().find(|blob| blob.id == id)
    }

    /// The number of records in each Catalog collection (blobs counted
    /// once each), for idempotency checks.
    pub fn counts(&self) -> [usize; 8] {
        [
            self.change_sets.len(),
            self.managed_parts.len(),
            self.part_revisions.len(),
            self.assets.len(),
            self.asset_revisions.len(),
            self.asset_blobs.len(),
            self.diagnostics.len(),
            self.audit_events.len(),
        ]
    }
}

/// Reads every Catalog record back, in a fixed order, and downloads every
/// blob. Fails if a stored file's bytes do not hash to its `sha256`.
pub fn read_back(client: &PocketBaseClient) -> CollabResult<CatalogReadback> {
    let mut readback = CatalogReadback {
        change_sets: client.list_records("catalog_change_sets", "", "seq")?,
        managed_parts: client.list_records("catalog_managed_parts", "", "ipn")?,
        part_revisions: client.list_records("catalog_part_revisions", "", "managed_part,number")?,
        assets: client.list_records("catalog_assets", "", "kind,key")?,
        asset_revisions: client.list_records("catalog_asset_revisions", "", "asset,number")?,
        asset_blobs: client.list_records("catalog_asset_blobs", "", "sha256")?,
        diagnostics: client.list_records("catalog_import_diagnostics", "", "code,location,id")?,
        audit_events: client.list_records("catalog_audit_events", "", "id")?,
    };
    if !readback.asset_blobs.is_empty() {
        let token = client.file_token()?;
        for blob in &mut readback.asset_blobs {
            blob.bytes =
                client.download_file("catalog_asset_blobs", &blob.id, &blob.file, Some(&token))?;
            if sha256_hex(&blob.bytes) != blob.sha256 || blob.bytes.len() as u64 != blob.size {
                return Err(CollabError::Protocol(format!(
                    "stored blob {} does not match its SHA-256 or size",
                    blob.sha256
                )));
            }
        }
    }
    Ok(readback)
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[((triple >> (18 - 6 * index)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_4648() {
        for (input, output) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), output);
        }
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }

    #[test]
    fn outcomes_parse() {
        let applied: ImportOutcome = serde_json::from_str(
            r#"{"outcome":"applied","correlationId":"c","changeSetId":"x","seq":1,"created":{"blobs":2,"assets":2,"assetRevisions":2,"managedParts":29,"partRevisions":29,"successorRevisions":0,"diagnostics":1},"unchanged":{}}"#,
        )
        .unwrap();
        assert!(matches!(applied, ImportOutcome::Applied { seq: 1, .. }));
        let noop: ImportOutcome =
            serde_json::from_str(r#"{"outcome":"noop","correlationId":"c","unchanged":{}}"#)
                .unwrap();
        assert!(matches!(noop, ImportOutcome::Noop { .. }));
    }
}
