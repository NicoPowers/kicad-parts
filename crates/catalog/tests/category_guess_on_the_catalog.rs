//! How well the category guess does on the 178 parts of the old CSV
//! catalog, leave one out: each part is guessed from only the clues a
//! downloaded part would give (its symbol's reference letter, its footprint
//! name, MPN, manufacturer and description), with that part taken out of
//! the examples. The clues are in `fixtures/category-guess/parts.json`.
//!
//! Run with `--nocapture` to see top-1 and top-3 accuracy per category.

use catalog::category::CategoryDetails;
use catalog::guess::{Clues, ExamplePart, guess_category};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// `fixtures/category-guess/parts.json`.
#[derive(Deserialize)]
struct Fixture {
    categories: Vec<FixtureCategory>,
    parts: Vec<FixturePart>,
}

#[derive(Deserialize)]
struct FixtureCategory {
    code: String,
    fields: Vec<String>,
}

#[derive(Deserialize)]
struct FixturePart {
    ipn: String,
    category: String,
    reference: String,
    footprints: Vec<String>,
    description: String,
    manufacturer: String,
    mpn: String,
}

fn fixture() -> Fixture {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/category-guess/parts.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn the_guess_finds_most_categories_in_its_top_three_leave_one_out() {
    let fixture = fixture();
    let categories: Vec<CategoryDetails> = fixture
        .categories
        .into_iter()
        .map(|category| CategoryDetails::starting(&category.code, category.fields))
        .collect();
    assert_eq!(categories.len(), 21);
    let parts: Vec<ExamplePart> = fixture
        .parts
        .into_iter()
        .map(|part| ExamplePart {
            ipn: part.ipn,
            category: part.category,
            reference: part.reference,
            footprints: part.footprints,
            description: part.description,
            manufacturers: vec![part.manufacturer],
            mpns: vec![part.mpn],
        })
        .collect();
    assert_eq!(parts.len(), 178);

    let (tally, misses) = measure(&categories, &parts, true);
    let percent = |part: u32, all: u32| 100.0 * f64::from(part) / f64::from(all.max(1));
    println!("category  parts  top-1  top-3");
    let (mut all, mut first, mut three) = (0, 0, 0);
    for (category, (count, top1, top3)) in &tally {
        println!(
            "{category:<8}  {count:>5}  {:>4.0}%  {:>4.0}%",
            percent(*top1, *count),
            percent(*top3, *count)
        );
        all += count;
        first += top1;
        three += top3;
    }
    println!(
        "overall   {all:>5}  {:>4.1}%  {:>4.1}%",
        percent(first, all),
        percent(three, all)
    );
    for miss in &misses {
        println!("not in the top three: {miss}");
    }
    // Downloads often carry no description; for information only.
    let (without, _) = measure(&categories, &parts, false);
    let (count, top1, top3) = without.values().fold((0, 0, 0), |sum, entry| {
        (sum.0 + entry.0, sum.1 + entry.1, sum.2 + entry.2)
    });
    println!(
        "without descriptions: top-1 {:.1}%, top-3 {:.1}%",
        percent(top1, count),
        percent(top3, count)
    );
    assert!(
        percent(three, all) >= 90.0,
        "top-3 accuracy is {:.1}%, below 90%",
        percent(three, all)
    );
}

/// Parts, right first and right in the top three, per category, and the
/// parts whose category was not in the top three.
type Tally = BTreeMap<String, (u32, u32, u32)>;

fn measure(
    categories: &[CategoryDetails],
    parts: &[ExamplePart],
    with_description: bool,
) -> (Tally, Vec<String>) {
    let mut tally: Tally = BTreeMap::new();
    let mut misses = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let others: Vec<ExamplePart> = parts
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, other)| other.clone())
            .collect();
        // What a download gives: one footprint's name, without its library.
        let footprint = part.footprints.first().map(|footprint| {
            footprint
                .rsplit_once(':')
                .map_or(footprint.as_str(), |(_, name)| name)
                .to_owned()
        });
        let clues = Clues {
            reference: part.reference.clone(),
            footprints: footprint.into_iter().collect(),
            description: if with_description {
                part.description.clone()
            } else {
                String::new()
            },
            manufacturer: part.manufacturers[0].clone(),
            mpn: part.mpns[0].clone(),
            ..Clues::default()
        };
        let ranked = guess_category(&clues, categories, &[], &others);
        // Ties count against the guess: the right category's place is how
        // many other categories score at least as much.
        let right = ranked
            .iter()
            .find(|guess| guess.category == part.category)
            .unwrap()
            .score;
        let position = ranked
            .iter()
            .filter(|guess| guess.category != part.category && guess.score >= right)
            .count();
        let entry = tally.entry(part.category.clone()).or_default();
        entry.0 += 1;
        if position == 0 && right > 0 {
            entry.1 += 1;
        }
        if position < 3 && right > 0 {
            entry.2 += 1;
        } else {
            misses.push(format!(
                "{} ({}) guessed {}",
                part.ipn,
                part.category,
                ranked
                    .iter()
                    .take(3)
                    .map(|guess| format!("{} {}", guess.category, guess.score))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    (tally, misses)
}
