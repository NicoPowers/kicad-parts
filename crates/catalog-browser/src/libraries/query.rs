//! Searching and filtering the index in memory.
//!
//! A search matches every word typed against an item's name, description
//! and keywords (ignoring case). Matches are ordered by how well the name
//! matches (the same, starting with the first word, holding it, or only
//! the description and keywords), then by group, library and name. Only
//! the first `limit` rows are sent, with the count of every match.

use super::Library;
use super::read::{FootprintItem, Items, SymbolItem};
use desktop_contract::{
    LibraryFilters, LibraryGroup, LibraryItemDetail, LibraryItemRow, LibraryItems,
    LibraryItemsRequest, LibraryKey, MountFilter, PartUse, PreviewKind, SymbolForFootprints,
};
use kicad_integration::footprint_filter::{matches_footprint_filters, matches_pin_count};
use std::collections::BTreeMap;

/// The Catalog parts using each reference (`LIBRARY:NAME`).
pub(crate) type Uses = BTreeMap<String, Vec<PartUse>>;

/// The largest page of rows sent.
pub(crate) const MAX_ROWS: usize = 500;

/// The rows `request` asks for. `symbols` is the symbol index, for
/// footprints listed for a symbol.
pub(crate) fn items(
    libraries: &[Library],
    symbols: &[Library],
    request: &LibraryItemsRequest,
    uses: &Uses,
    reading: bool,
) -> LibraryItems {
    let query = request.query.trim().to_lowercase();
    let terms: Vec<&str> = query.split_whitespace().collect();
    let for_symbol = match request.kind {
        PreviewKind::Footprint => request
            .for_symbol
            .as_deref()
            .filter(|reference| !reference.trim().is_empty())
            .map(|reference| symbol_for(symbols, reference.trim())),
        PreviewKind::Symbol => None,
    };
    let mut matches: Vec<(u8, usize, usize)> = Vec::new();
    let chosen: Vec<&Library> = libraries
        .iter()
        .filter(|library| {
            request
                .library
                .as_ref()
                .is_none_or(|key| *key == library.source.key)
        })
        .collect();
    for (at, library) in chosen.iter().enumerate() {
        let Some(loaded) = &library.loaded else {
            continue;
        };
        for (index, text) in loaded.search.iter().enumerate() {
            if !terms.iter().all(|term| text.contains(term)) {
                continue;
            }
            let passes = match &loaded.items {
                Items::Symbols(items) => symbol_passes(&items[index], &request.filters),
                Items::Footprints(items) => footprint_passes(
                    &items[index],
                    &library.source.key.nickname,
                    &request.filters,
                    for_symbol.as_ref(),
                ),
            };
            if passes {
                let name = text.split('\u{1f}').next().unwrap_or_default();
                matches.push((rank(name, &query, &terms), at, index));
            }
        }
    }
    let total = matches.len();
    let name_of = |&(_, at, index): &(u8, usize, usize)| -> (LibraryGroup, String, String) {
        let library: &Library = chosen[at];
        let name = match &library
            .loaded
            .as_ref()
            .expect("matched items are loaded")
            .items
        {
            Items::Symbols(items) => &items[index].name,
            Items::Footprints(items) => &items[index].name,
        };
        (
            library.source.key.group,
            library.source.key.nickname.to_lowercase(),
            name.to_lowercase(),
        )
    };
    let limit = request.limit.min(MAX_ROWS);
    let key = |item: &(u8, usize, usize)| (item.0, name_of(item));
    if matches.len() > limit {
        matches.select_nth_unstable_by_key(limit, key);
        matches.truncate(limit);
    }
    matches.sort_by_cached_key(key);
    let rows = matches
        .into_iter()
        .map(|(_, at, index)| {
            let library = chosen[at];
            let loaded = library.loaded.as_ref().expect("matched items are loaded");
            let (name, description) = match &loaded.items {
                Items::Symbols(items) => (&items[index].name, &items[index].description),
                Items::Footprints(items) => (&items[index].name, &items[index].description),
            };
            let reference = format!("{}:{name}", library.source.key.nickname);
            LibraryItemRow {
                library: library.source.key.clone(),
                name: name.clone(),
                description: description.clone(),
                in_catalog: uses.contains_key(&reference),
            }
        })
        .collect();
    LibraryItems {
        total,
        rows,
        for_symbol,
        reading,
    }
}

fn rank(name: &str, query: &str, terms: &[&str]) -> u8 {
    match terms.first() {
        None => 0,
        Some(_) if name == query => 0,
        Some(first) if name.starts_with(first) => 1,
        Some(first) if name.contains(first) => 2,
        Some(_) => 3,
    }
}

/// The reference letters of a symbol's Reference, such as `R` for `R?`.
pub(crate) fn reference_letter(reference: &str) -> &str {
    reference.trim_end_matches(|c: char| c == '?' || c.is_ascii_digit())
}

fn symbol_passes(item: &SymbolItem, filters: &LibraryFilters) -> bool {
    if filters.pins_min.is_some_and(|min| item.pins < min)
        || filters.pins_max.is_some_and(|max| item.pins > max)
        || (filters.hide_power && item.power)
        || (filters.with_footprint && item.footprint.trim().is_empty())
    {
        return false;
    }
    let wanted = filters.reference.trim();
    wanted.is_empty() || reference_letter(&item.reference).eq_ignore_ascii_case(wanted)
}

fn footprint_passes(
    item: &FootprintItem,
    library: &str,
    filters: &LibraryFilters,
    for_symbol: Option<&SymbolForFootprints>,
) -> bool {
    if filters.pads_min.is_some_and(|min| item.pads < min)
        || filters.pads_max.is_some_and(|max| item.pads > max)
    {
        return false;
    }
    let mount = match filters.mount {
        MountFilter::Any => true,
        MountFilter::Smd => item.mount == "smd",
        MountFilter::ThroughHole => item.mount == "tht",
    };
    if !mount {
        return false;
    }
    let Some(symbol) = for_symbol.filter(|symbol| symbol.found) else {
        return true;
    };
    (!filters.match_footprint_filters
        || matches_footprint_filters(&symbol.footprint_filters, library, &item.name))
        && (!filters.match_pin_count || matches_pin_count(symbol.pin_count, item.pads))
}

/// The symbol `reference` names, looked up by nickname as KiCad would:
/// ours first, then the person's installed libraries, then KiCad's.
pub(crate) fn symbol_for(symbols: &[Library], reference: &str) -> SymbolForFootprints {
    let missing = SymbolForFootprints {
        reference: reference.to_owned(),
        footprint_filters: Vec::new(),
        pin_count: 0,
        found: false,
    };
    let Some((nickname, name)) = reference.split_once(':') else {
        return missing;
    };
    for group in [
        LibraryGroup::Ours,
        LibraryGroup::Installed,
        LibraryGroup::Kicad,
    ] {
        let key = LibraryKey {
            group,
            nickname: nickname.to_owned(),
        };
        if let Some(SymbolOrFootprint::Symbol(item)) = find_item(symbols, &key, name) {
            return SymbolForFootprints {
                reference: reference.to_owned(),
                footprint_filters: item.filters.clone(),
                pin_count: item.pins,
                found: true,
            };
        }
    }
    missing
}

pub(crate) enum SymbolOrFootprint<'a> {
    Symbol(&'a SymbolItem),
    Footprint(&'a FootprintItem),
}

/// The item `name` of the library `key`, if it is read.
pub(crate) fn find_item<'a>(
    libraries: &'a [Library],
    key: &LibraryKey,
    name: &str,
) -> Option<SymbolOrFootprint<'a>> {
    let library = libraries
        .iter()
        .find(|library| library.source.key == *key)?;
    match &library.loaded.as_ref()?.items {
        Items::Symbols(items) => items
            .iter()
            .find(|item| item.name == name)
            .map(SymbolOrFootprint::Symbol),
        Items::Footprints(items) => items
            .iter()
            .find(|item| item.name == name)
            .map(SymbolOrFootprint::Footprint),
    }
}

/// One item's details.
pub(crate) fn detail(
    libraries: &[Library],
    key: &LibraryKey,
    name: &str,
    uses: &Uses,
) -> Option<LibraryItemDetail> {
    let library = libraries
        .iter()
        .find(|library| library.source.key == *key)?;
    let reference = format!("{}:{name}", key.nickname);
    let used_by = uses.get(&reference).cloned().unwrap_or_default();
    let file = library.source.path.display().to_string();
    Some(match find_item(libraries, key, name)? {
        SymbolOrFootprint::Symbol(item) => LibraryItemDetail {
            reference,
            description: item.description.clone(),
            keywords: item.keywords.clone(),
            file,
            count: item.pins,
            reference_letter: reference_letter(&item.reference).to_owned(),
            power: item.power,
            default_footprint: item.footprint.clone(),
            footprint_filters: item.filters.clone(),
            used_by,
            ..LibraryItemDetail::default()
        },
        SymbolOrFootprint::Footprint(item) => LibraryItemDetail {
            reference,
            description: item.description.clone(),
            keywords: item.keywords.clone(),
            file,
            count: item.pads,
            mount: match item.mount.as_str() {
                "smd" => "SMD",
                "tht" => "Through-hole",
                _ => "",
            }
            .to_owned(),
            model: item.model.clone().unwrap_or_default(),
            used_by,
            ..LibraryItemDetail::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_letters_drop_question_marks_and_numbers() {
        assert_eq!(reference_letter("R?"), "R");
        assert_eq!(reference_letter("U12"), "U");
        assert_eq!(reference_letter("#PWR"), "#PWR");
    }

    #[test]
    fn symbols_pass_each_filter() {
        let item = SymbolItem {
            name: "R".into(),
            reference: "R".into(),
            pins: 2,
            ..SymbolItem::default()
        };
        let filters = |change: fn(&mut LibraryFilters)| {
            let mut filters = LibraryFilters::default();
            change(&mut filters);
            filters
        };
        assert!(symbol_passes(&item, &LibraryFilters::default()));
        assert!(symbol_passes(&item, &filters(|f| f.pins_min = Some(2))));
        assert!(!symbol_passes(&item, &filters(|f| f.pins_min = Some(3))));
        assert!(!symbol_passes(&item, &filters(|f| f.pins_max = Some(1))));
        assert!(symbol_passes(&item, &filters(|f| f.reference = "r".into())));
        assert!(!symbol_passes(
            &item,
            &filters(|f| f.reference = "C".into())
        ));
        assert!(!symbol_passes(&item, &filters(|f| f.with_footprint = true)));
        let power = SymbolItem {
            power: true,
            ..item.clone()
        };
        assert!(!symbol_passes(&power, &filters(|f| f.hide_power = true)));
    }

    #[test]
    fn footprints_pass_pads_mount_and_the_symbols_filters() {
        let item = FootprintItem {
            name: "R_0603_1608Metric".into(),
            pads: 2,
            mount: "smd".into(),
            ..FootprintItem::default()
        };
        let mut filters = LibraryFilters::default();
        assert!(footprint_passes(&item, "Resistor_SMD", &filters, None));
        filters.mount = MountFilter::ThroughHole;
        assert!(!footprint_passes(&item, "Resistor_SMD", &filters, None));
        filters.mount = MountFilter::Smd;
        filters.pads_min = Some(3);
        assert!(!footprint_passes(&item, "Resistor_SMD", &filters, None));
        filters.pads_min = Some(2);
        let symbol = SymbolForFootprints {
            reference: "Device:R".into(),
            footprint_filters: vec!["R_*".into()],
            pin_count: 2,
            found: true,
        };
        filters.match_footprint_filters = true;
        filters.match_pin_count = true;
        assert!(footprint_passes(
            &item,
            "Resistor_SMD",
            &filters,
            Some(&symbol)
        ));
        let three = SymbolForFootprints {
            pin_count: 3,
            ..symbol.clone()
        };
        assert!(!footprint_passes(
            &item,
            "Resistor_SMD",
            &filters,
            Some(&three)
        ));
        let capacitor = FootprintItem {
            name: "C_0603_1608Metric".into(),
            ..item.clone()
        };
        assert!(!footprint_passes(
            &capacitor,
            "Capacitor_SMD",
            &filters,
            Some(&symbol)
        ));
        // The filters are off: everything passes.
        filters.match_footprint_filters = false;
        filters.match_pin_count = false;
        assert!(footprint_passes(
            &capacitor,
            "Capacitor_SMD",
            &filters,
            Some(&three)
        ));
    }
}
