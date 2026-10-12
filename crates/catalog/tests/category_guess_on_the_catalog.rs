//! How well the category guess does on the real CSV catalog, leave one out:
//! each part is guessed from only the clues a downloaded part would give
//! (its symbol's reference letter, its footprint name, MPN, manufacturer
//! and description), with that part taken out of the examples. The CSV
//! catalog is only read.
//!
//! Run with `--nocapture` to see top-1 and top-3 accuracy per category.

use catalog::category::{CategoryDetails, category_code};
use catalog::csv_import::{ColumnRoute, parse_csv, read_input, route_column};
use catalog::guess::{Clues, ExamplePart, guess_category};
use kicad_integration::library_file::{SymbolLibrary, SymbolLookup, symbol_reference};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The reference of `g-<library>:<name>`, read from the CSV catalog's own
/// symbol library (following `extends`). KiCad's own libraries are not in
/// the repository, so their symbols give no reference: a pessimistic
/// stand-in for a download, which always has one.
fn reference_of(
    root: &Path,
    symbol: &str,
    libraries: &mut BTreeMap<String, Option<Vec<u8>>>,
) -> String {
    let Some((library, name)) = symbol.split_once(':') else {
        return String::new();
    };
    if !library.starts_with("g-") {
        return String::new();
    }
    let bytes = libraries
        .entry(library.to_owned())
        .or_insert_with(|| read_input(root, &format!("symbols/{library}.kicad_sym")).ok());
    let Some(bytes) = bytes.as_ref() else {
        return String::new();
    };
    let Ok(parsed) = SymbolLibrary::parse(bytes) else {
        return String::new();
    };
    let mut name = name.to_owned();
    for _ in 0..4 {
        let SymbolLookup::Found(entry) = parsed.find(&name) else {
            return String::new();
        };
        if let Some(reference) = symbol_reference(&bytes[entry.span.clone()]) {
            return reference;
        }
        match &entry.extends {
            Some(parent) => name.clone_from(parent),
            None => return String::new(),
        }
    }
    String::new()
}

#[test]
fn the_guess_finds_most_categories_in_its_top_three_leave_one_out() {
    let root = repository();
    let mut csv_files: Vec<String> = std::fs::read_dir(root.join("database"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("g-") && name.ends_with(".csv"))
        .map(|name| name.trim_end_matches(".csv").to_owned())
        .collect();
    csv_files.sort();
    assert_eq!(csv_files.len(), 21);

    let mut libraries = BTreeMap::new();
    let mut categories = Vec::new();
    let mut parts = Vec::new();
    for csv_file in &csv_files {
        let path = format!("database/{csv_file}.csv");
        let table = parse_csv(&path, &read_input(&root, &path).unwrap()).unwrap();
        let code = category_code(csv_file).to_owned();
        let fields = table
            .headers
            .iter()
            .filter(|header| route_column(header) == ColumnRoute::PartDetails)
            .cloned()
            .collect();
        categories.push(CategoryDetails::starting(&code, fields));
        let column = |name: &str| table.headers.iter().position(|header| header == name);
        for record in &table.records {
            let value = |name: &str| {
                column(name)
                    .and_then(|index| record.fields.get(index))
                    .cloned()
                    .unwrap_or_default()
            };
            let symbol = value("Symbol");
            let symbol = symbol.split(';').next().unwrap_or_default().trim();
            parts.push(ExamplePart {
                ipn: value("IPN"),
                category: code.clone(),
                reference: reference_of(&root, symbol, &mut libraries),
                footprints: value("Footprint")
                    .split(';')
                    .map(str::trim)
                    .filter(|footprint| !footprint.is_empty())
                    .map(str::to_owned)
                    .collect(),
                description: value("Description"),
                manufacturers: vec![value("Manufacturer")],
                mpns: vec![value("MPN")],
            });
        }
    }
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
        let position = ranked
            .iter()
            .position(|guess| guess.category == part.category)
            .unwrap();
        let entry = tally.entry(part.category.clone()).or_default();
        entry.0 += 1;
        if position == 0 && ranked[0].score > 0 {
            entry.1 += 1;
        }
        if position < 3 && ranked[position].score > 0 {
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
