//! Guesses a new part's category from the clues a downloaded part offers.
//!
//! [`guess_category`] ranks every category, each with a score and the
//! reasons for it in plain words. It is a pure function: the same inputs
//! always give the same answer, and it uses no network. Clues count in
//! layers, most reliable first:
//!
//! 1. matches people confirmed earlier ([`RememberedMatch`]), and the same
//!    MPN already in the Catalog;
//! 2. footprint name patterns (KiCad names such as `R_0603*`, IPC-7351
//!    names such as `RESC*`);
//! 3. the symbol's reference letter, with aliases (`IC` is `U`); `U` says
//!    nothing;
//! 4. keywords in the description, and the closest existing parts, which
//!    vote: description words, footprint family, manufacturer and MPN;
//! 5. a supplier category, but only through a remembered match: on its own
//!    it never moves a part to a category.
//!
//! The person always confirms. When no clue matches, every score is 0 and
//! [`best_guess`] gives `None`.

use crate::category::{CategoryDetails, footprint_family, reference_letter};
use std::collections::{BTreeMap, BTreeSet};

/// What a downloaded part says about itself. Blank text and empty lists
/// mean the clue is missing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Clues {
    /// The symbol's reference, such as `R` or `U?`.
    pub reference: String,
    /// Footprint names, with or without their library.
    pub footprints: Vec<String>,
    pub symbol: String,
    pub description: String,
    pub keywords: String,
    pub manufacturer: String,
    pub mpn: String,
    /// A DigiKey, Mouser or LCSC category, such as `Chip Resistor - Surface
    /// Mount`. It counts only through a remembered match.
    pub supplier_category: String,
}

/// The kind of clue a remembered match is for.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ClueKind {
    SupplierCategory,
    FootprintFamily,
    ReferenceLetter,
    Manufacturer,
}

impl ClueKind {
    pub const ALL: [Self; 4] = [
        Self::SupplierCategory,
        Self::FootprintFamily,
        Self::ReferenceLetter,
        Self::Manufacturer,
    ];

    /// The name stored with a remembered match (catalog_category_matches
    /// clue).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SupplierCategory => "supplier-category",
            Self::FootprintFamily => "footprint-family",
            Self::ReferenceLetter => "reference-letter",
            Self::Manufacturer => "manufacturer",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    fn words(self) -> &'static str {
        match self {
            Self::SupplierCategory => "supplier category",
            Self::FootprintFamily => "footprint family",
            Self::ReferenceLetter => "reference letter",
            Self::Manufacturer => "manufacturer",
        }
    }

    /// The most a remembered match of this kind adds, when every match for
    /// the clue is for one category and no existing part has the clue.
    /// Otherwise it adds this times the category's matches over all matches
    /// and existing parts with the clue.
    fn weight(self) -> u32 {
        match self {
            Self::SupplierCategory | Self::FootprintFamily => 150,
            Self::ReferenceLetter => 100,
            Self::Manufacturer => 50,
        }
    }
}

/// A person confirmed `category` for a part with this clue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RememberedMatch {
    pub category: String,
    pub clue: ClueKind,
    pub value: String,
}

/// An existing part, which the guess compares a new one with.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExamplePart {
    pub ipn: String,
    /// Its category's code.
    pub category: String,
    pub reference: String,
    pub footprints: Vec<String>,
    pub description: String,
    pub manufacturers: Vec<String>,
    pub mpns: Vec<String>,
}

/// One category's place in the ranking.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Guess {
    pub category: String,
    pub name: String,
    pub score: u32,
    /// Why, strongest first, in plain words.
    pub reasons: Vec<String>,
}

// How much each kind of clue adds. Each layer outweighs the next.
const SAME_MPN: u32 = 1000;
const FOOTPRINT_PATTERN: u32 = 100;
const REFERENCE_LETTER: u32 = 60;
const FIRST_KEYWORD: u32 = 30;
const MORE_KEYWORDS: u32 = 10;
const MOST_KEYWORDS: u32 = 50;
/// How many of the closest parts vote.
const NEIGHBOURS: usize = 5;
/// The most the closest parts' votes add, together.
const MOST_VOTES: u32 = 60;

/// Every category in `categories`, ranked by how well `clues` fit it:
/// highest score first, then in the order given. See the module
/// documentation for the layers.
pub fn guess_category(
    clues: &Clues,
    categories: &[CategoryDetails],
    remembered: &[RememberedMatch],
    examples: &[ExamplePart],
) -> Vec<Guess> {
    let mut scores: BTreeMap<&str, Score> = categories
        .iter()
        .map(|category| (category.code.as_str(), Score::default()))
        .collect();
    let clue = ClueSet::of(clues);

    // 1. The same MPN already in the Catalog, and remembered matches.
    if !clue.mpn.is_empty() {
        for example in examples {
            if example.mpns.iter().any(|mpn| normalize(mpn) == clue.mpn) {
                if let Some(score) = scores.get_mut(example.category.as_str()) {
                    score.add(
                        SAME_MPN,
                        format!(
                            "MPN {} is already {} in this category",
                            clues.mpn.trim(),
                            example.ipn
                        ),
                    );
                }
            }
        }
    }
    for kind in ClueKind::ALL {
        let Some(value) = clue.value_of(kind) else {
            continue;
        };
        let matching: Vec<&RememberedMatch> = remembered
            .iter()
            .filter(|remembered| remembered.clue == kind && normalize(&remembered.value) == value)
            .collect();
        // Existing parts with the same clue count in the share too, so a
        // remembered match is evidence on top of the parts, never over them:
        // one person's move of one resistor does not outweigh 28 others.
        let parts_with_clue = examples
            .iter()
            .filter(|example| example_has(example, kind, &value))
            .count() as u32;
        let share_of = (matching.len() as u32 + parts_with_clue).max(1);
        let mut per_category: BTreeMap<&str, u32> = BTreeMap::new();
        for remembered in &matching {
            *per_category
                .entry(remembered.category.as_str())
                .or_default() += 1;
        }
        for (category, count) in per_category {
            if let Some(score) = scores.get_mut(category) {
                let times = if count == 1 {
                    "once".to_owned()
                } else {
                    format!("{count} times")
                };
                score.add(
                    kind.weight() * count / share_of,
                    format!(
                        "people chose this category {times} for {} {}",
                        kind.words(),
                        clue.shown(kind)
                    ),
                );
            }
        }
    }

    for category in categories {
        let score = scores
            .get_mut(category.code.as_str())
            .expect("every category has a score");
        // 2. Footprint name patterns.
        let pattern_match = clue.footprint_names.iter().find_map(|name| {
            category
                .rules
                .footprint_patterns
                .iter()
                .find(|pattern| matches_pattern(pattern, name))
                .map(|pattern| (name, pattern))
        });
        if let Some((name, pattern)) = pattern_match {
            score.add(
                FOOTPRINT_PATTERN,
                format!("footprint {name} matches {pattern}"),
            );
        }
        // 3. The reference letter.
        if !clue.letter.is_empty()
            && category
                .rules
                .reference_letters
                .iter()
                .any(|letter| reference_letter(letter) == clue.letter)
        {
            score.add(
                REFERENCE_LETTER,
                format!("reference letter {}", clue.letter),
            );
        }
        // 4a. Keywords in the description.
        let found: Vec<&String> = category
            .rules
            .keywords
            .iter()
            .filter(|keyword| clue.has_phrase(keyword))
            .collect();
        if let Some(first) = found.first() {
            let points =
                (FIRST_KEYWORD + MORE_KEYWORDS * (found.len() as u32 - 1)).min(MOST_KEYWORDS);
            score.add(points, format!("the description says {first}"));
        }
    }

    // 4b. The closest existing parts vote.
    let mut neighbours: Vec<(u32, &ExamplePart)> = examples
        .iter()
        .map(|example| (similarity(&clue, example), example))
        .filter(|(points, _)| *points > 0)
        .collect();
    neighbours.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.ipn.cmp(&b.1.ipn)));
    neighbours.truncate(NEIGHBOURS);
    let mut votes: BTreeMap<&str, (u32, Vec<&str>)> = BTreeMap::new();
    for (points, example) in &neighbours {
        let vote = votes.entry(example.category.as_str()).or_default();
        vote.0 += points;
        vote.1.push(example.ipn.as_str());
    }
    for (category, (points, ipns)) in votes {
        let Some(score) = scores.get_mut(category) else {
            continue;
        };
        let added = (points * MOST_VOTES / (NEIGHBOURS as u32 * 100)).max(1);
        let reason = match same_family_count(&clue, examples, category) {
            Some((family, count)) => format!(
                "footprint family {family} matches {count} {} in this category",
                if count == 1 { "part" } else { "parts" }
            ),
            None if ipns.len() == 1 => format!("similar to {}", ipns[0]),
            None => format!("similar to {} parts here, such as {}", ipns.len(), ipns[0]),
        };
        score.add(added, reason);
    }

    let mut ranked: Vec<Guess> = categories
        .iter()
        .map(|category| {
            let score = scores.remove(category.code.as_str()).unwrap_or_default();
            let mut reasons = score.reasons;
            reasons.sort_by(|a, b| b.0.cmp(&a.0));
            Guess {
                category: category.code.clone(),
                name: category.name.clone(),
                score: score.total,
                reasons: reasons.into_iter().map(|(_, reason)| reason).collect(),
            }
        })
        .collect();
    // A stable sort keeps the order given for equal scores.
    ranked.sort_by(|a, b| b.score.cmp(&a.score));
    ranked
}

/// The first guess, if any clue matched.
pub fn best_guess(ranked: &[Guess]) -> Option<&Guess> {
    ranked.first().filter(|guess| guess.score > 0)
}

#[derive(Default)]
struct Score {
    total: u32,
    reasons: Vec<(u32, String)>,
}

impl Score {
    fn add(&mut self, points: u32, reason: String) {
        if points == 0 {
            return;
        }
        self.total += points;
        self.reasons.push((points, reason));
    }
}

/// The clues, worked out once.
struct ClueSet {
    letter: String,
    footprint_names: Vec<String>,
    families: BTreeSet<String>,
    words: BTreeSet<String>,
    /// The description, symbol name and keywords as lowercase words
    /// between single spaces, with a space at each end.
    text: String,
    manufacturer: String,
    mpn: String,
    supplier_category: String,
    shown_family: String,
    shown_manufacturer: String,
    shown_supplier_category: String,
}

impl ClueSet {
    fn of(clues: &Clues) -> Self {
        let footprint_names: Vec<String> = clues
            .footprints
            .iter()
            .map(|footprint| footprint_name(footprint).to_owned())
            .filter(|name| !name.is_empty())
            .collect();
        let families: BTreeSet<String> = footprint_names
            .iter()
            .map(|name| normalize(&footprint_family(name)))
            .filter(|family| !family.is_empty())
            .collect();
        let shown_family = footprint_names
            .iter()
            .map(|name| footprint_family(name))
            .find(|family| !family.is_empty())
            .unwrap_or_default();
        let joined = format!("{} {} {}", clues.description, clues.symbol, clues.keywords);
        Self {
            letter: reference_letter(&clues.reference),
            footprint_names,
            families,
            words: words(&joined),
            text: format!(" {} ", spaced(&joined)),
            manufacturer: manufacturer_key(&clues.manufacturer),
            mpn: normalize(&clues.mpn),
            supplier_category: normalize(&clues.supplier_category),
            shown_family,
            shown_manufacturer: clues.manufacturer.trim().to_owned(),
            shown_supplier_category: clues.supplier_category.trim().to_owned(),
        }
    }

    /// The value a remembered match of `kind` is compared with, normalized.
    fn value_of(&self, kind: ClueKind) -> Option<String> {
        let value = match kind {
            ClueKind::SupplierCategory => self.supplier_category.clone(),
            ClueKind::FootprintFamily => normalize(&self.shown_family),
            ClueKind::ReferenceLetter => normalize(&self.letter),
            ClueKind::Manufacturer => normalize(&self.shown_manufacturer),
        };
        (!value.is_empty()).then_some(value)
    }

    fn shown(&self, kind: ClueKind) -> &str {
        match kind {
            ClueKind::SupplierCategory => &self.shown_supplier_category,
            ClueKind::FootprintFamily => &self.shown_family,
            ClueKind::ReferenceLetter => &self.letter,
            ClueKind::Manufacturer => &self.shown_manufacturer,
        }
    }

    fn has_phrase(&self, phrase: &str) -> bool {
        let phrase = spaced(phrase);
        !phrase.is_empty() && self.text.contains(&format!(" {phrase} "))
    }
}

/// Whether `example` has the clue of `kind` with the normalized `value`.
/// Existing parts have no supplier category.
fn example_has(example: &ExamplePart, kind: ClueKind, value: &str) -> bool {
    match kind {
        ClueKind::SupplierCategory => false,
        ClueKind::FootprintFamily => example
            .footprints
            .iter()
            .any(|footprint| normalize(&footprint_family(footprint)) == value),
        ClueKind::ReferenceLetter => normalize(&reference_letter(&example.reference)) == value,
        ClueKind::Manufacturer => example
            .manufacturers
            .iter()
            .any(|manufacturer| normalize(manufacturer) == value),
    }
}

/// How alike a new part and an existing one are, from 0 to about 100.
fn similarity(clue: &ClueSet, example: &ExamplePart) -> u32 {
    let mut points = 0;
    let theirs = words(&example.description);
    if !clue.words.is_empty() && !theirs.is_empty() {
        let shared = clue.words.intersection(&theirs).count() as u32;
        let all = clue.words.union(&theirs).count() as u32;
        points += 50 * shared / all;
    }
    let names: BTreeSet<String> = example
        .footprints
        .iter()
        .map(|footprint| normalize(footprint_name(footprint)))
        .collect();
    if clue
        .footprint_names
        .iter()
        .any(|name| names.contains(&normalize(name)))
    {
        points += 10;
    }
    if example.footprints.iter().any(|footprint| {
        clue.families
            .contains(&normalize(&footprint_family(footprint)))
    }) {
        points += 15;
    }
    if !clue.manufacturer.is_empty()
        && example
            .manufacturers
            .iter()
            .any(|manufacturer| manufacturer_key(manufacturer) == clue.manufacturer)
    {
        points += 10;
    }
    if example
        .mpns
        .iter()
        .any(|mpn| shared_prefix(&normalize(mpn), &clue.mpn) >= 4)
    {
        points += 10;
    }
    if !clue.letter.is_empty() && reference_letter(&example.reference) == clue.letter {
        points += 5;
    }
    points
}

/// The new part's footprint family and how many parts of `category` share
/// it, when any do.
fn same_family_count(
    clue: &ClueSet,
    examples: &[ExamplePart],
    category: &str,
) -> Option<(String, usize)> {
    let family = normalize(&clue.shown_family);
    if family.is_empty() {
        return None;
    }
    let count = examples
        .iter()
        .filter(|example| example.category == category)
        .filter(|example| {
            example
                .footprints
                .iter()
                .any(|footprint| normalize(&footprint_family(footprint)) == family)
        })
        .count();
    (count > 0).then(|| (clue.shown_family.clone(), count))
}

/// A footprint's name without its library.
fn footprint_name(footprint: &str) -> &str {
    footprint
        .rsplit_once(':')
        .map_or(footprint, |(_, name)| name)
        .trim()
}

/// Whether `name` fits `pattern`, where `*` stands for any text, ignoring
/// case.
fn matches_pattern(pattern: &str, name: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let name = name.to_lowercase();
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, rest) = parts.split_first().expect("split gives at least one part");
    let Some(mut remaining) = name.strip_prefix(first) else {
        return false;
    };
    let Some((last, middle)) = rest.split_last() else {
        return remaining.is_empty();
    };
    for part in middle {
        match remaining.find(part) {
            Some(at) => remaining = &remaining[at + part.len()..],
            None => return false,
        }
    }
    remaining.ends_with(last)
}

/// Lowercase letters and digits only.
fn normalize(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Lowercase words separated by single spaces.
fn spaced(text: &str) -> String {
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Words that tell parts apart: at least two characters, not only digits,
/// and not one of a few filler words.
fn words(text: &str) -> BTreeSet<String> {
    const FILLER: [&str; 8] = ["and", "for", "with", "the", "of", "to", "in", "smd"];
    spaced(text)
        .split(' ')
        .filter(|word| word.chars().count() >= 2)
        .filter(|word| !word.chars().all(|ch| ch.is_ascii_digit()))
        .filter(|word| !FILLER.contains(word))
        .map(str::to_owned)
        .collect()
}

/// The first word of a manufacturer's name, which survives most spellings
/// (`Analog Devices Inc.` and `Analog Devices`).
fn manufacturer_key(manufacturer: &str) -> String {
    spaced(manufacturer)
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn shared_prefix(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::category::CategoryDetails;

    fn categories() -> Vec<CategoryDetails> {
        ["res", "cap", "reg", "ics"]
            .into_iter()
            .map(CategoryDetails::starting)
            .collect()
    }

    fn example(ipn: &str, category: &str, footprint: &str, description: &str) -> ExamplePart {
        ExamplePart {
            ipn: ipn.into(),
            category: category.into(),
            reference: "U".into(),
            footprints: vec![footprint.into()],
            description: description.into(),
            manufacturers: vec!["Texas Instruments".into()],
            mpns: vec![format!("MPN-{ipn}")],
        }
    }

    #[test]
    fn a_footprint_pattern_and_reference_letter_settle_a_passive() {
        let clues = Clues {
            reference: "C".into(),
            footprints: vec!["CAPC1608X90N".into()],
            ..Clues::default()
        };
        let ranked = guess_category(&clues, &categories(), &[], &[]);
        assert_eq!(ranked.len(), 4);
        let best = best_guess(&ranked).unwrap();
        assert_eq!(best.category, "cap");
        assert_eq!(best.score, FOOTPRINT_PATTERN + REFERENCE_LETTER);
        assert_eq!(
            best.reasons,
            ["footprint CAPC1608X90N matches CAPC*", "reference letter C"]
        );
        // The same inputs always give the same answer.
        assert_eq!(guess_category(&clues, &categories(), &[], &[]), ranked);
    }

    /// 29 Yageo resistors on R_0603 with reference R, the first moved to
    /// `first_in`, and the clues recorded each time a person moved it.
    fn resistors(first_in: &str) -> Vec<ExamplePart> {
        (0..29)
            .map(|n| ExamplePart {
                ipn: format!("RES-0000-{n:04}"),
                category: if n == 0 { first_in } else { "res" }.into(),
                reference: "R".into(),
                footprints: vec!["g-Resistor_SMD:R_0603_1608Metric".into()],
                description: format!("RES {n}K OHM 1% 1/10W 0603"),
                manufacturers: vec!["Yageo".into()],
                mpns: vec![format!("RC0603FR-07{n}KL")],
            })
            .collect()
    }

    fn confirmed(category: &str) -> Vec<RememberedMatch> {
        [
            (ClueKind::FootprintFamily, "R"),
            (ClueKind::ReferenceLetter, "R"),
            (ClueKind::Manufacturer, "Yageo"),
        ]
        .into_iter()
        .map(|(clue, value)| RememberedMatch {
            category: category.into(),
            clue,
            value: value.into(),
        })
        .collect()
    }

    #[test]
    fn one_confirmation_is_evidence_on_top_of_the_parts_not_over_them() {
        let clues = Clues {
            reference: "R".into(),
            footprints: vec!["R_0402_1005Metric".into()],
            manufacturer: "Yageo".into(),
            ..Clues::default()
        };
        let order = |ranked: Vec<Guess>| -> Vec<String> {
            ranked
                .into_iter()
                .take(3)
                .map(|guess| guess.category)
                .collect()
        };
        let before = order(guess_category(
            &clues,
            &categories(),
            &[],
            &resistors("res"),
        ));
        assert_eq!(before[0], "res");
        // One resistor moved to capacitors does not make every resistor a
        // capacitor: 28 resistors still say otherwise.
        let moved = guess_category(&clues, &categories(), &confirmed("cap"), &resistors("cap"));
        assert_eq!(moved[0].category, "res", "{moved:?}");
        let capacitors = moved.iter().find(|guess| guess.category == "cap").unwrap();
        assert!(capacitors.score < moved[0].score / 4, "{moved:?}");
        // Moving it back restores the guess.
        let mut both = confirmed("cap");
        both.extend(confirmed("res"));
        let back = order(guess_category(
            &clues,
            &categories(),
            &both,
            &resistors("res"),
        ));
        assert_eq!(back, before);
    }

    #[test]
    fn no_clue_leaves_the_guess_empty_in_the_order_given() {
        let ranked = guess_category(&Clues::default(), &categories(), &[], &[]);
        assert!(best_guess(&ranked).is_none());
        let order: Vec<&str> = ranked.iter().map(|guess| guess.category.as_str()).collect();
        assert_eq!(order, ["res", "cap", "reg", "ics"]);
        // U says nothing.
        let chip = Clues {
            reference: "IC1".into(),
            ..Clues::default()
        };
        assert!(best_guess(&guess_category(&chip, &categories(), &[], &[])).is_none());
    }

    #[test]
    fn the_closest_parts_tell_chip_categories_apart() {
        let examples = [
            example(
                "REG-0001-0000",
                "reg",
                "SOT-23-5",
                "Linear Voltage Regulator Positive Fixed 250mA",
            ),
            example(
                "REG-0002-0000",
                "reg",
                "SOT-23-5",
                "Linear Voltage Regulator Positive Fixed 100mA",
            ),
            example(
                "ICS-0001-0000",
                "ics",
                "SOT-23-5",
                "Buffer Non-Inverting 5.5V",
            ),
        ];
        let clues = Clues {
            reference: "U".into(),
            footprints: vec!["Package_TO_SOT_SMD:SOT-23-5".into()],
            description: "Linear Voltage Regulator 3.3V 500mA".into(),
            ..Clues::default()
        };
        let ranked = guess_category(&clues, &categories(), &[], &examples);
        let best = best_guess(&ranked).unwrap();
        assert_eq!(best.category, "reg");
        assert_eq!(best.reasons[0], "the description says regulator");
        assert!(
            best.reasons
                .contains(&"footprint family SOT matches 2 parts in this category".to_owned())
        );
        // The same MPN already in the Catalog outweighs everything.
        let same_mpn = Clues {
            mpn: "mpn-ics-0001-0000".into(),
            ..clues
        };
        let best = guess_category(&same_mpn, &categories(), &[], &examples).remove(0);
        assert_eq!(best.category, "ics");
        assert_eq!(
            best.reasons[0],
            "MPN mpn-ics-0001-0000 is already ICS-0001-0000 in this category"
        );
    }

    #[test]
    fn a_supplier_category_counts_only_through_a_remembered_match() {
        let clues = Clues {
            supplier_category: "Chip Resistor - Surface Mount".into(),
            ..Clues::default()
        };
        // On its own, a supplier category moves nothing.
        assert!(best_guess(&guess_category(&clues, &categories(), &[], &[])).is_none());
        // Once a person confirmed it for a category, it counts there.
        let remembered = [
            RememberedMatch {
                category: "res".into(),
                clue: ClueKind::SupplierCategory,
                value: "chip resistor - surface mount".into(),
            },
            RememberedMatch {
                category: "res".into(),
                clue: ClueKind::SupplierCategory,
                value: "Chip Resistor - Surface Mount".into(),
            },
            RememberedMatch {
                category: "cap".into(),
                clue: ClueKind::SupplierCategory,
                value: "Chip Resistor - Surface Mount".into(),
            },
        ];
        let ranked = guess_category(&clues, &categories(), &remembered, &[]);
        assert_eq!(ranked[0].category, "res");
        assert_eq!(ranked[0].score, 100);
        assert_eq!(
            ranked[0].reasons,
            [
                "people chose this category 2 times for supplier category Chip Resistor - Surface Mount"
            ]
        );
        assert_eq!(ranked[1].category, "cap");
        assert_eq!(ranked[1].score, 50);
        assert_eq!(
            ClueKind::parse("footprint-family"),
            Some(ClueKind::FootprintFamily)
        );
    }

    #[test]
    fn patterns_match_whole_names_ignoring_case() {
        assert!(matches_pattern("R_*", "R_0603_1608Metric"));
        assert!(matches_pattern("resc*", "RESC1608X55N"));
        assert!(!matches_pattern("R_*", "RV_Trim"));
        assert!(matches_pattern("*Shield*", "RF_Shield_Frame"));
        assert!(matches_pattern("LED", "led"));
        assert!(!matches_pattern("LED", "LED_0603"));
    }
}
