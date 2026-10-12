use csv_catalog::{CatalogPart, CategorySummary, CsvCatalog};
use std::path::Path;
use std::time::{Duration, Instant};

/// Product target for interactive search: a query over 20,000 rows must
/// answer well inside one frame budget of typing feedback. Tests run in the
/// unoptimized profile, so the bound here is deliberately loose; release
/// builds are an order of magnitude faster.
const SEARCH_BUDGET: Duration = Duration::from_millis(100);

fn repo_root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

#[test]
fn loads_every_csv_catalog_category_and_row() {
    let catalog = CsvCatalog::load(repo_root()).expect("CSV catalog loads");
    assert_eq!(catalog.categories().len(), 21);
    assert_eq!(catalog.parts().len(), 178);
    let rows: usize = catalog.categories().iter().map(|c| c.rows).sum();
    assert_eq!(rows, 178);

    let resistor = catalog.find("RES-0000-0470").expect("known resistor");
    assert_eq!(resistor.category, "g-res");
    assert_eq!(resistor.mpn, "RC0603FR-0747RL");
    assert_eq!(resistor.symbol.as_deref(), Some("g-res:R_US"));
    assert_eq!(
        resistor.footprint.as_deref(),
        Some("g-Resistor_SMD:R_0603_1608Metric")
    );
}

#[test]
fn real_catalog_search_finds_parts_by_mpn_and_description() {
    let catalog = CsvCatalog::load(repo_root()).expect("CSV catalog loads");
    let results = catalog.search("RC0603FR-0747RL", 10);
    assert_eq!(results.total, 1);
    assert_eq!(catalog.parts()[results.hits[0]].ipn, "RES-0000-0470");
    assert!(catalog.search("0603", 500).total > 10);
}

fn synthetic_catalog(rows: usize) -> CsvCatalog {
    let packages = ["0402", "0603", "0805", "1206", "SOT-23", "SOIC-8", "QFN-32"];
    let makers = ["Yageo", "Murata", "TI", "ST", "Vishay", "Nexperia"];
    let parts = (0..rows)
        .map(|n| {
            let package = packages[n % packages.len()];
            let maker = makers[n % makers.len()];
            CatalogPart {
                ipn: format!("SYN-{:04}-{:04}", n / 10_000, n % 10_000),
                category: format!("g-syn{}", n % 21),
                mpn: format!("MPN{n:06}{package}"),
                manufacturer: maker.into(),
                description: format!("Synthetic part {n} in {package} from {maker}"),
                value: format!("{}k", n % 1000),
                symbol: Some(format!("g-syn:SYM_{}", n % 97)),
                footprint: Some(format!("g-syn:{package}")),
                datasheet: String::new(),
                fields: Vec::new(),
            }
        })
        .collect();
    CsvCatalog::from_parts(
        parts,
        vec![CategorySummary {
            name: "g-syn".into(),
            rows,
        }],
    )
}

#[test]
fn searches_twenty_thousand_rows_within_budget() {
    let catalog = synthetic_catalog(20_000);
    let queries = [
        "",
        "0603",
        "murata 0805",
        "MPN012345",
        "syn-0001-9999",
        "nothing-matches",
    ];
    // Warm caches once so the measurement reflects steady-state typing.
    for query in queries {
        catalog.search(query, 100);
    }
    let mut slowest = Duration::ZERO;
    for query in queries {
        let started = Instant::now();
        let results = catalog.search(query, 100);
        slowest = slowest.max(started.elapsed());
        assert!(results.hits.len() <= 100);
    }
    eprintln!("slowest 20,000-row search: {slowest:?}");
    assert_eq!(catalog.search("", 100).total, 20_000);
    assert_eq!(catalog.search("MPN012345", 100).total, 1);
    assert_eq!(catalog.search("nothing-matches", 100).total, 0);
    assert!(
        slowest < SEARCH_BUDGET,
        "slowest 20,000-row search took {slowest:?}, budget {SEARCH_BUDGET:?}"
    );
}
