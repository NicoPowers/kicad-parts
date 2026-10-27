//! The parts in `parts.toml`, and the starting categories they are in.

use catalog::category::starting_categories;
use catalog::{CategoryDetails, DetailField, ManufacturerPart};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Everything `parts.toml` lists, in its order, and the categories its
/// parts are in.
#[derive(Clone, Debug)]
pub struct Sample {
    /// The starting categories the parts are in, in the order the parts
    /// first name them, as every new team server has them.
    pub categories: Vec<CategoryDetails>,
    pub parts: Vec<SamplePart>,
}

/// `parts.toml` as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PartsFile {
    #[serde(rename = "part")]
    parts: Vec<SamplePart>,
}

/// One part, with the values of some of its category's fields.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SamplePart {
    pub ipn: String,
    pub category: String,
    /// Manufacturer and MPN pairs, the main one first.
    #[serde(default)]
    pub manufacturer_parts: Vec<(String, String)>,
    /// `LIB:NAME`, or blank.
    #[serde(default)]
    pub symbol: String,
    /// `LIB:NAME`, several choices separated by `;`, or blank.
    #[serde(default)]
    pub footprint: String,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}

impl Sample {
    /// The parts of the sample library in this repository.
    pub fn load() -> Self {
        Self::read(&crate::root()).expect("fixtures/sample-library/parts.toml is readable")
    }

    /// The parts of the sample library at `root`.
    pub fn read(root: &Path) -> Result<Self, String> {
        let path = root.join("parts.toml");
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let file: PartsFile =
            toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        let starting = starting_categories();
        let mut categories: Vec<CategoryDetails> = Vec::new();
        for part in &file.parts {
            let category = starting
                .iter()
                .find(|category| category.code == part.category)
                .ok_or_else(|| format!("{} names no starting category", part.ipn))?;
            if !categories.iter().any(|known| known.code == category.code) {
                categories.push(category.clone());
            }
            if let Some(name) = part
                .fields
                .keys()
                .find(|name| !category.fields.contains(name))
            {
                return Err(format!(
                    "{} gives {name}, which is not a field of {}",
                    part.ipn, category.code
                ));
            }
        }
        Ok(Self {
            categories,
            parts: file.parts,
        })
    }

    /// The part `ipn`; panics if there is none, as a test should.
    pub fn part(&self, ipn: &str) -> &SamplePart {
        self.parts
            .iter()
            .find(|part| part.ipn == ipn)
            .unwrap_or_else(|| panic!("parts.toml has no part {ipn}"))
    }

    /// The category `code`; panics if no part is in it.
    pub fn category(&self, code: &str) -> &CategoryDetails {
        self.categories
            .iter()
            .find(|category| category.code == code)
            .unwrap_or_else(|| panic!("no part of parts.toml is in the category {code}"))
    }

    /// The parts in the categories `codes`, in order; every part when
    /// `codes` is empty.
    pub fn parts_in<'a>(&'a self, codes: &'a [&str]) -> impl Iterator<Item = &'a SamplePart> {
        self.parts
            .iter()
            .filter(move |part| codes.is_empty() || codes.contains(&part.category.as_str()))
    }
}

impl SamplePart {
    /// The value of field `name`, blank if the part gives none.
    pub fn field(&self, name: &str) -> &str {
        self.fields.get(name).map_or("", String::as_str)
    }

    /// The main MPN, blank if the part has none.
    pub fn mpn(&self) -> &str {
        self.manufacturer_parts
            .first()
            .map_or("", |(_, mpn)| mpn.as_str())
    }

    /// Every field of `category`, in its order, with this part's values.
    pub fn detail_fields(&self, category: &CategoryDetails) -> Vec<DetailField> {
        category
            .fields
            .iter()
            .map(|name| DetailField {
                name: name.clone(),
                value: self.field(name).to_owned(),
            })
            .collect()
    }

    pub fn manufacturer_parts(&self) -> Vec<ManufacturerPart> {
        self.manufacturer_parts
            .iter()
            .map(|(manufacturer, mpn)| ManufacturerPart {
                manufacturer: manufacturer.clone(),
                mpn: mpn.clone(),
            })
            .collect()
    }

    /// Symbol, then Footprint, as a part revision has them.
    pub fn cad_fields(&self) -> Vec<DetailField> {
        vec![
            DetailField {
                name: "Symbol".into(),
                value: self.symbol.clone(),
            },
            DetailField {
                name: "Footprint".into(),
                value: self.footprint.clone(),
            },
        ]
    }

    /// The footprint choices, in order.
    pub fn footprints(&self) -> Vec<&str> {
        self.footprint
            .split(';')
            .map(str::trim)
            .filter(|choice| !choice.is_empty())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_part_names_a_category_and_only_its_fields() {
        let sample = Sample::load();
        assert!(!sample.parts.is_empty());
        let mut ipns: Vec<&str> = sample.parts.iter().map(|part| part.ipn.as_str()).collect();
        ipns.sort();
        ipns.dedup();
        assert_eq!(ipns.len(), sample.parts.len(), "each IPN once");
        for part in &sample.parts {
            assert!(catalog::Ipn::parse(&part.ipn).is_ok(), "{}", part.ipn);
            let category = sample.category(&part.category);
            assert!(
                part.ipn.starts_with(&category.ipn_prefix),
                "{} is not in {}'s numbering",
                part.ipn,
                category.code
            );
        }
    }
}
