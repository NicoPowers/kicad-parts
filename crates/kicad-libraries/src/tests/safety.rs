//! An update while another connection reads the database, the way KiCad
//! does through ODBC. KiCad's background refresh reads one table at a
//! time, outside a transaction (`backgroundRefreshWorker` in
//! `eeschema/sch_io/database/sch_io_database.cpp`): each table it reads is
//! whole, before the update or after it, never half changed. Across the
//! tables of one refresh, a moving row can show in two groups or in none;
//! KiCad's `LoadSymbol` then looks in every table, so links still resolve.

use super::{resistors, team, test_folder};
use crate::update;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

const TABLES: [&str; 3] = ["category_res", "older_revisions", "archived_revisions"];

/// One table's entries, read on their own.
fn table(connection: &Connection, table: &str) -> Vec<String> {
    let mut statement = connection
        .prepare(&format!("SELECT \"Entry\" FROM \"{table}\" ORDER BY 1"))
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Looks `entry` up in each table in turn, as `LoadSymbol` does.
fn found_in(connection: &Connection, entry: &str) -> Option<&'static str> {
    TABLES.into_iter().find(|table| {
        connection
            .query_row(
                &format!("SELECT count(*) FROM \"{table}\" WHERE \"Entry\" = ?1"),
                [entry],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0
    })
}

fn open(folder: &Path) -> Connection {
    Connection::open_with_flags(
        folder.join("MA-Parts.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
}

#[test]
fn a_reader_in_a_transaction_sees_the_old_state_until_it_ends() {
    let root = test_folder("safety");
    let folder = root.join("MA");
    let mut fixture = resistors();
    {
        let mut blobs = fixture.blobs();
        update(&root, &fixture.catalog, &team(), &mut blobs).unwrap();
    }
    let reader = open(&folder);
    // The reader holds a read transaction open across the update, as a
    // slow query would.
    reader.execute_batch("BEGIN").unwrap();
    assert_eq!(table(&reader, "older_revisions"), ["RES-0000-0470_v1"]);
    fixture.archive("RES-0000-0470", 2, true);
    {
        let mut blobs = fixture.blobs();
        let outcome = update(&root, &fixture.catalog, &team(), &mut blobs).unwrap();
        assert!(
            outcome.database_changed,
            "the update does not wait for the reader"
        );
    }
    // Inside its transaction the reader still sees the old state.
    assert_eq!(
        table(&reader, "category_res"),
        ["RES-0000-0100_v1", "RES-0000-0470_v2"]
    );
    assert!(table(&reader, "archived_revisions").is_empty());
    reader.execute_batch("COMMIT").unwrap();
    // Its next reads see the new state: one row moved, none lost.
    assert_eq!(
        table(&reader, "category_res"),
        ["RES-0000-0100_v1", "RES-0000-0470_v1"]
    );
    assert_eq!(table(&reader, "archived_revisions"), ["RES-0000-0470_v2"]);
    assert!(table(&reader, "older_revisions").is_empty());
}

#[test]
fn tables_read_one_at_a_time_are_each_whole_and_lookups_find_every_entry() {
    let root = test_folder("safety-many");
    let folder = root.join("MA");
    let mut fixture = resistors();
    {
        let mut blobs = fixture.blobs();
        update(&root, &fixture.catalog, &team(), &mut blobs).unwrap();
    }
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reading = {
        let (done, folder) = (done.clone(), folder.clone());
        std::thread::spawn(move || {
            let reader = open(&folder);
            let mut reads = 0;
            while !done.load(std::sync::atomic::Ordering::SeqCst) || reads < 10 {
                // Each table, read on its own, is one of its two whole
                // states: revision 2 current, or archived.
                let current = table(&reader, "category_res");
                assert!(
                    current == ["RES-0000-0100_v1", "RES-0000-0470_v2"]
                        || current == ["RES-0000-0100_v1", "RES-0000-0470_v1"],
                    "{current:?}"
                );
                let older = table(&reader, "older_revisions");
                assert!(
                    older.is_empty() || older == ["RES-0000-0470_v1"],
                    "{older:?}"
                );
                let archived = table(&reader, "archived_revisions");
                assert!(
                    archived.is_empty() || archived == ["RES-0000-0470_v2"],
                    "{archived:?}"
                );
                reads += 1;
            }
            reads
        })
    };
    for round in 0..20 {
        fixture.archive("RES-0000-0470", 2, round % 2 == 0);
        let mut blobs = fixture.blobs();
        update(&root, &fixture.catalog, &team(), &mut blobs).unwrap();
    }
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(reading.join().unwrap() >= 10);
    // Between updates, a lookup in every table finds every entry.
    let reader = open(&folder);
    for entry in ["RES-0000-0100_v1", "RES-0000-0470_v1", "RES-0000-0470_v2"] {
        assert!(found_in(&reader, entry).is_some(), "{entry}");
    }
}
