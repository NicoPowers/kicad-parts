//! A small Catalog built by hand, and tests of the libraries made from it.

mod layout;
mod safety;

use crate::{Naming, Team};
use catalog::pocketbase::{
    CatalogReadback, StoredArchiveEntry, StoredBlob, StoredCadItem, StoredCadItemRevision,
    StoredCategoryRevision, StoredChange, StoredPart, StoredPartDetails, StoredPartRevision,
};
use catalog::{
    BlobId, CadItemDetails, CadItemId, CadItemKind, CadItemRevisionId, CadReference,
    CadReferenceRole, CadReferenceTarget, CategoryDetails, DetailField, License, ManufacturerPart,
    ModelReference, ModelTarget, PART_DETAILS_SCHEMA, PART_REVISION_SCHEMA, PartDetails,
    PartDetailsId, PartId, PartRevision, PartRevisionId, sha256_hex,
};
use std::path::{Path, PathBuf};

/// The team the tests write for, with code `MA`.
pub fn team() -> Team {
    Team {
        id: "team-settings-ma".into(),
        server: "http://127.0.0.1:8090".into(),
        naming: Naming::new("MA").unwrap(),
    }
}

/// A folder under `target/` for one test, emptied first.
pub fn test_folder(name: &str) -> PathBuf {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/kicad-libraries-tests")
        .join(name);
    clear_read_only(&folder);
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    std::path::absolute(folder).unwrap()
}

/// Clears the read-only mark of every file under `folder`, so it can be
/// deleted or changed.
pub fn clear_read_only(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            clear_read_only(&path);
        } else {
            let _ = crate::files::set_read_only(&path, false);
        }
    }
}

/// A symbol entry as the CSV import stores one: the exact bytes of one
/// `(symbol ...)`, units included.
pub fn symbol_bytes(name: &str, body: &str) -> Vec<u8> {
    format!(
        "(symbol \"{name}\"\n\t\t(exclude_from_sim no)\n\t\t(in_bom yes)\n\t\t(on_board yes)\n\t\t(property \"Reference\" \"R\"\n\t\t\t(at 2.032 0 90)\n\t\t\t(effects\n\t\t\t\t(font\n\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t)\n\t\t\t)\n\t\t)\n\t\t(property \"Value\" \"{name}\"\n\t\t\t(at 0 0 90)\n\t\t\t(effects\n\t\t\t\t(font\n\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t)\n\t\t\t)\n\t\t)\n\t\t(symbol \"{name}_0_1\"\n\t\t\t(rectangle\n\t\t\t\t(start -1.016 -2.54)\n\t\t\t\t(end 1.016 2.54)\n\t\t\t\t(stroke\n\t\t\t\t\t(width 0.254)\n\t\t\t\t\t(type default)\n\t\t\t\t)\n\t\t\t\t(fill\n\t\t\t\t\t(type none)\n\t\t\t\t)\n\t\t\t)\n\t\t)\n\t\t(symbol \"{name}_1_1\"\n\t\t\t(pin passive line\n\t\t\t\t(at 0 3.81 270)\n\t\t\t\t(length 1.27)\n\t\t\t\t(name \"~\"\n\t\t\t\t\t(effects\n\t\t\t\t\t\t(font\n\t\t\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t\t\t)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t\t(number \"1\"\n\t\t\t\t\t(effects\n\t\t\t\t\t\t(font\n\t\t\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t\t\t)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t)\n\t\t\t(pin passive line\n\t\t\t\t(at 0 -3.81 90)\n\t\t\t\t(length 1.27)\n\t\t\t\t(name \"~\"\n\t\t\t\t\t(effects\n\t\t\t\t\t\t(font\n\t\t\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t\t\t)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t\t(number \"2\"\n\t\t\t\t\t(effects\n\t\t\t\t\t\t(font\n\t\t\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t\t\t)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t)\n\t\t){body}\n\t)"
    )
    .into_bytes()
}

/// A footprint with two pads and the 3D model links `models`.
pub fn footprint_bytes(name: &str, models: &[&str]) -> Vec<u8> {
    let mut text = format!(
        "(footprint \"{name}\"\n\t(version 20241229)\n\t(generator \"pcbnew\")\n\t(layer \"F.Cu\")\n\t(attr smd)\n\t(pad \"1\" smd roundrect\n\t\t(at -0.825 0)\n\t\t(size 0.8 0.95)\n\t\t(layers \"F.Cu\" \"F.Mask\" \"F.Paste\")\n\t\t(roundrect_rratio 0.25)\n\t)\n\t(pad \"2\" smd roundrect\n\t\t(at 0.825 0)\n\t\t(size 0.8 0.95)\n\t\t(layers \"F.Cu\" \"F.Mask\" \"F.Paste\")\n\t\t(roundrect_rratio 0.25)\n\t)\n"
    );
    for model in models {
        text.push_str(&format!(
            "\t(model \"{model}\"\n\t\t(offset\n\t\t\t(xyz 0 0 0)\n\t\t)\n\t\t(scale\n\t\t\t(xyz 1 1 1)\n\t\t)\n\t\t(rotate\n\t\t\t(xyz 0 0 0)\n\t\t)\n\t)\n"
        ));
    }
    text.push_str(")\n");
    text.into_bytes()
}

/// A Catalog built by hand: categories, CAD items with revisions, parts
/// with details and revisions, and archive entries.
#[derive(Default)]
pub struct Fixture {
    pub catalog: CatalogReadback,
    seq: u64,
    /// The KiCad file version new symbol revisions say they were saved in.
    pub symbol_version: Option<String>,
}

/// A CAD item revision a part or footprint names.
#[derive(Clone)]
pub struct Bound {
    pub cad_item: String,
    pub number: u32,
    pub snapshot_sha256: String,
    pub key: String,
}

impl Fixture {
    /// Resistors (with a Tolerance field) and voltage regulators.
    pub fn new() -> Self {
        let mut fixture = Self::default();
        fixture.category(
            "res",
            vec!["Description", "Value", "Tolerance", "Datasheet"],
        );
        fixture.category(
            "reg",
            vec!["Description", "Voltage", "Current", "Datasheet"],
        );
        fixture
    }

    pub fn category(&mut self, code: &str, fields: Vec<&str>) {
        let details =
            CategoryDetails::starting(code, fields.into_iter().map(str::to_owned).collect());
        let number = self
            .catalog
            .category_revisions
            .iter()
            .filter(|revision| revision.category == code)
            .count() as u32
            + 1;
        self.catalog
            .category_revisions
            .push(StoredCategoryRevision {
                id: format!("{code}-{number}"),
                category: code.to_owned(),
                number,
                predecessor: String::new(),
                change: String::new(),
                snapshot: details.canonical_json(),
                snapshot_sha256: details.sha256(),
                source: String::new(),
            });
    }

    /// Records the next Change and returns its ID.
    fn change(&mut self) -> String {
        self.seq += 1;
        let id = format!("change-{}", self.seq);
        self.catalog.changes.push(StoredChange {
            id: id.clone(),
            seq: self.seq,
            kind: "edit".into(),
            reason: String::new(),
            correlation_id: id.clone(),
            actor: String::new(),
            actor_name: String::new(),
            created: String::new(),
            source_commit: String::new(),
            sources: String::new(),
            summary: String::new(),
        });
        id
    }

    fn blob(&mut self, bytes: Vec<u8>) -> String {
        let sha256 = sha256_hex(&bytes);
        let id = BlobId::new(&sha256).to_string();
        if !self.catalog.blobs.iter().any(|blob| blob.id == id) {
            self.catalog.blobs.push(StoredBlob {
                id: id.clone(),
                sha256,
                size: bytes.len() as u64,
                file: "blob".into(),
                first_change: String::new(),
                bytes,
            });
        }
        id
    }

    /// Adds the next revision of the CAD item `key` with `bytes`.
    fn cad(
        &mut self,
        kind: CadItemKind,
        key: &str,
        format: &str,
        bytes: Vec<u8>,
        models: Vec<ModelReference>,
    ) -> Bound {
        let id = CadItemId::new(kind, key).to_string();
        if !self.catalog.cad_items.iter().any(|item| item.id == id) {
            let change = self.change();
            self.catalog.cad_items.push(StoredCadItem {
                id: id.clone(),
                kind: kind.as_str().to_owned(),
                key: key.to_owned(),
                first_change: change,
            });
        }
        let number = self
            .catalog
            .cad_item_revisions
            .iter()
            .filter(|revision| revision.cad_item == id)
            .count() as u32
            + 1;
        let details = CadItemDetails {
            schema: "kicad-parts/cad-item-details/1".into(),
            kind,
            key: key.to_owned(),
            format: format.to_owned(),
            format_version: match kind {
                CadItemKind::Symbol => Some(
                    self.symbol_version
                        .clone()
                        .unwrap_or_else(|| "20241209".to_owned()),
                ),
                CadItemKind::Footprint => Some("20241229".to_owned()),
                CadItemKind::Model3d => None,
            },
            blob_sha256: sha256_hex(&bytes),
            blob_size: bytes.len() as u64,
            model_references: models,
            license: License::UnknownNeedsReview,
        };
        let blob = self.blob(bytes);
        self.catalog.cad_item_revisions.push(StoredCadItemRevision {
            id: CadItemRevisionId::new(&CadItemId::from_stored(&id), number).to_string(),
            cad_item: id.clone(),
            number,
            predecessor: String::new(),
            change: String::new(),
            blob,
            snapshot_sha256: details.sha256(),
            snapshot: details.canonical_json(),
            license: "unknown-needs-review".into(),
            source: String::new(),
        });
        Bound {
            cad_item: id,
            number,
            snapshot_sha256: details.sha256(),
            key: key.to_owned(),
        }
    }

    pub fn symbol(&mut self, key: &str, body: &str) -> Bound {
        let name = key.split_once(':').unwrap().1;
        self.cad(
            CadItemKind::Symbol,
            key,
            "kicad-symbol-entry",
            symbol_bytes(name, body),
            Vec::new(),
        )
    }

    pub fn model(&mut self, key: &str, bytes: &[u8]) -> Bound {
        self.cad(
            CadItemKind::Model3d,
            key,
            "3d-model",
            bytes.to_vec(),
            Vec::new(),
        )
    }

    /// A footprint revision whose links are `models`: each a link as
    /// written and the model revision it binds, if any.
    pub fn footprint(&mut self, key: &str, models: &[(&str, Option<&Bound>)]) -> Bound {
        let name = key.split_once(':').unwrap().1;
        let links: Vec<&str> = models.iter().map(|(link, _)| *link).collect();
        let references = models
            .iter()
            .map(|(link, bound)| ModelReference {
                reference: (*link).to_owned(),
                scope: "external-variable".into(),
                target: match bound {
                    Some(bound) => ModelTarget::CadItemRevision {
                        cad_item: bound.cad_item.clone(),
                        number: bound.number,
                        snapshot_sha256: bound.snapshot_sha256.clone(),
                    },
                    None => ModelTarget::Unresolved {
                        status: "external".into(),
                    },
                },
            })
            .collect();
        self.cad(
            CadItemKind::Footprint,
            key,
            "kicad-footprint",
            footprint_bytes(name, &links),
            references,
        )
    }

    /// Adds a part with its first details.
    pub fn part(&mut self, ipn: &str, category: &str, fields: &[(&str, &str)], mpn: &str) {
        let part = PartId::for_text(ipn).to_string();
        let details = PartDetails {
            schema: PART_DETAILS_SCHEMA.into(),
            ipn: ipn.into(),
            category: category.into(),
            fields: fields
                .iter()
                .map(|(name, value)| DetailField {
                    name: (*name).into(),
                    value: (*value).into(),
                })
                .collect(),
            manufacturer_parts: vec![ManufacturerPart {
                manufacturer: "Yageo".into(),
                mpn: mpn.into(),
            }],
        };
        let details_id = PartDetailsId::new(&PartId::from_stored(&part), 1).to_string();
        self.catalog.part_details.push(StoredPartDetails {
            id: details_id.clone(),
            part: part.clone(),
            number: 1,
            predecessor: String::new(),
            change: String::new(),
            snapshot: details.canonical_json(),
            snapshot_sha256: details.sha256(),
            source: String::new(),
        });
        self.catalog.parts.push(StoredPart {
            id: part,
            ipn: ipn.into(),
            ipn_prefix: ipn[..3].into(),
            variant: String::new(),
            current_revision: String::new(),
            current_revision_number: 0,
            current_details: details_id,
            current_details_number: 1,
            first_change: String::new(),
        });
    }

    /// Adds the next revision of `ipn`, using `symbol` and `footprints`
    /// (each a bound revision or a KiCad library reference).
    pub fn revision(
        &mut self,
        ipn: &str,
        symbol: Cad<'_>,
        footprints: &[Cad<'_>],
        excluded_from_bom: bool,
    ) -> u32 {
        let part = PartId::for_text(ipn).to_string();
        let number = self
            .catalog
            .part_revisions
            .iter()
            .filter(|revision| revision.part == part)
            .count() as u32
            + 1;
        let reference = |role, ordinal, cad: &Cad<'_>| CadReference {
            role,
            ordinal,
            reference: match cad {
                Cad::Bound(bound) => bound.key.clone(),
                Cad::KiCad(reference) => (*reference).to_owned(),
            },
            target: match cad {
                Cad::Bound(bound) => CadReferenceTarget::CadItemRevision {
                    cad_item: bound.cad_item.clone(),
                    number: bound.number,
                    snapshot_sha256: bound.snapshot_sha256.clone(),
                },
                Cad::KiCad(_) => CadReferenceTarget::External,
            },
        };
        let mut cad_references = vec![reference(CadReferenceRole::Symbol, 0, &symbol)];
        for (ordinal, footprint) in footprints.iter().enumerate() {
            cad_references.push(reference(
                CadReferenceRole::Footprint,
                ordinal as u32,
                footprint,
            ));
        }
        let revision = PartRevision {
            schema: PART_REVISION_SCHEMA.into(),
            ipn: ipn.into(),
            cad_fields: Vec::new(),
            cad_references,
            excluded_from_bom,
        };
        let id = PartRevisionId::new(&PartId::from_stored(&part), number).to_string();
        self.catalog.part_revisions.push(StoredPartRevision {
            id: id.clone(),
            part: part.clone(),
            number,
            predecessor: String::new(),
            change: String::new(),
            snapshot: revision.canonical_json(),
            snapshot_sha256: revision.sha256(),
            source: String::new(),
            cad_item_revisions: Vec::new(),
        });
        if let Some(stored) = self.catalog.parts.iter_mut().find(|p| p.id == part) {
            stored.current_revision = id;
            stored.current_revision_number = number;
        }
        number
    }

    /// Archives (or unarchives) revision `number` of `ipn`.
    pub fn archive(&mut self, ipn: &str, number: u32, archived: bool) {
        self.seq += 1;
        let part = PartId::for_text(ipn).to_string();
        self.catalog.archive_entries.push(StoredArchiveEntry {
            id: format!("archive-{}", self.seq),
            part: part.clone(),
            part_revision: PartRevisionId::new(&PartId::from_stored(&part), number).to_string(),
            revision_number: number,
            archived,
            change: String::new(),
            seq: self.seq,
            current_revision_after: String::new(),
            current_revision_number_after: 0,
        });
    }

    /// The stored bytes of each blob, as a blob source.
    pub fn blobs(&self) -> impl FnMut(&StoredBlob) -> Result<Vec<u8>, String> + use<'_> {
        |blob: &StoredBlob| {
            self.catalog
                .blobs
                .iter()
                .find(|stored| stored.id == blob.id)
                .map(|stored| stored.bytes.clone())
                .ok_or_else(|| "no such blob".to_owned())
        }
    }
}

/// What a part revision uses for a symbol or footprint.
pub enum Cad<'a> {
    Bound(&'a Bound),
    /// A KiCad library item, such as `Resistor_SMD:R_0402_1005Metric`.
    KiCad(&'a str),
}

/// The resistor catalog most tests start from: RES-0000-0470 with
/// revision 1 (symbol R_US 1, footprint R_0603 1 with a model) and
/// revision 2 (symbol R_US 2), so revision 1 is older; and RES-0000-0100,
/// which uses KiCad's own footprint and is excluded from the BOM.
pub fn resistors() -> Fixture {
    let mut fixture = Fixture::new();
    let symbol_1 = fixture.symbol("g-res:R_US", "");
    let symbol_2 = fixture.symbol("g-res:R_US", "\n\t\t(embedded_fonts no)");
    let model = fixture.model(
        "3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step",
        b"ISO-10303-21; a model\n",
    );
    let footprint = fixture.footprint(
        "g-Resistor_SMD:R_0603_1608Metric",
        &[(
            "${GITPLM_PARTS}/3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step",
            Some(&model),
        )],
    );
    fixture.part(
        "RES-0000-0470",
        "res",
        &[
            ("Description", "RES 47 OHM 1% 1/10W 0603"),
            ("Value", "47"),
            ("Tolerance", "1%"),
            ("Datasheet", "https://example.test/47.pdf"),
        ],
        "RC0603FR-0747RL",
    );
    fixture.revision(
        "RES-0000-0470",
        Cad::Bound(&symbol_1),
        &[Cad::Bound(&footprint)],
        false,
    );
    fixture.revision(
        "RES-0000-0470",
        Cad::Bound(&symbol_2),
        &[Cad::Bound(&footprint)],
        false,
    );
    fixture.part(
        "RES-0000-0100",
        "res",
        &[
            ("Description", "RES 10 OHM 0402"),
            ("Value", ""),
            ("Tolerance", "5%"),
        ],
        "RC0402JR-0710RL",
    );
    fixture.revision(
        "RES-0000-0100",
        Cad::Bound(&symbol_1),
        &[Cad::KiCad("Resistor_SMD:R_0402_1005Metric")],
        true,
    );
    fixture
}
