//! The parts and categories in `parts.toml`.

use catalog::{CategoryDetails, DetailField, ManufacturerPart};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Everything `parts.toml` lists, in its order.
#[derive(Clone, Debug, Deserialize)]
pub struct Sample {
    #[serde(rename = "category")]
    pub categories: Vec<SampleCategory>,
    #[serde(rename = "part")]
    pub parts: Vec<SamplePart>,
}

/// One category: its code and its fields, in order.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleCategory {
    pub code: String,
    pub fields: Vec<String>,
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
        let sample: Self =
            toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        for part in &sample.parts {
            let category = sample
                .categories
                .iter()
                .find(|category| category.code == part.category)
                .ok_or_else(|| format!("{} names no category of parts.toml", part.ipn))?;
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
        Ok(sample)
    }

    /// The part `ipn`; panics if there is none, as a test should.
    pub fn part(&self, ipn: &str) -> &SamplePart {
        self.parts
            .iter()
            .find(|part| part.ipn == ipn)
            .unwrap_or_else(|| panic!("parts.toml has no part {ipn}"))
    }

    /// The category `code`; panics if there is none.
    pub fn category(&self, code: &str) -> &SampleCategory {
        self.categories
            .iter()
            .find(|category| category.code == code)
            .unwrap_or_else(|| panic!("parts.toml has no category {code}"))
    }

    /// The parts in the categories `codes`, in order; every part when
    /// `codes` is empty.
    pub fn parts_in<'a>(&'a self, codes: &'a [&str]) -> impl Iterator<Item = &'a SamplePart> {
        self.parts
            .iter()
            .filter(move |part| codes.is_empty() || codes.contains(&part.category.as_str()))
    }
}

impl SampleCategory {
    /// The category as a team server keeps it: its fields, and the name, IPN
    /// prefix and rules of the app's starting categories.
    pub fn details(&self) -> CategoryDetails {
        CategoryDetails::starting(&self.code, self.fields.clone())
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
    pub fn detail_fields(&self, category: &SampleCategory) -> Vec<DetailField> {
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
                part.ipn.starts_with(&category.details().ipn_prefix),
                "{} is not in {}'s numbering",
                part.ipn,
                category.code
            );
        }
    }
}
