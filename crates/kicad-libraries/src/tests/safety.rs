//! An update while another connection reads the database, as KiCad does
//! through ODBC: the reader sees the old rows or the new ones, never a mix.

use super::{resistors, test_folder};
use crate::{DATABASE_FILE, update};
use rusqlite::{Connection, OpenFlags};

/// Every entry in every table, in one read.
fn snapshot(connection: &Connection) -> Vec<(String, String)> {
    let mut statement = connection
        .prepare(
            "SELECT 'category_res', \"Entry\" FROM category_res \
             UNION ALL SELECT 'older_revisions', \"Entry\" FROM older_revisions \
             UNION ALL SELECT 'archived_revisions', \"Entry\" FROM archived_revisions \
             ORDER BY 1, 2",
        )
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn a_reader_sees_the_old_state_or_the_new_never_a_mix() {
    let folder = test_folder("safety");
    let mut fixture = resistors();
    {
        let mut blobs = fixture.blobs();
        update(&folder, &fixture.catalog, &mut blobs).unwrap();
    }
    let reader =
        Connection::open_with_flags(folder.join(DATABASE_FILE), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let old = snapshot(&reader);
    assert_eq!(
        old,
        [
            ("category_res".to_owned(), "RES-0000-0100_v1".to_owned()),
            ("category_res".to_owned(), "RES-0000-0470_v2".to_owned()),
            ("older_revisions".to_owned(), "RES-0000-0470_v1".to_owned()),
        ]
    );

    // The reader holds a read transaction open across the update, as a
    // slow query would.
    reader.execute_batch("BEGIN").unwrap();
    assert_eq!(snapshot(&reader), old);
    fixture.archive("RES-0000-0470", 2, true);
    {
        let mut blobs = fixture.blobs();
        let outcome = update(&folder, &fixture.catalog, &mut blobs).unwrap();
        assert!(
            outcome.database_changed,
            "the update does not wait for the reader"
        );
    }
    // Inside its transaction the reader still sees the whole old state.
    assert_eq!(snapshot(&reader), old);
    reader.execute_batch("COMMIT").unwrap();
    // Its next read sees the whole new state: one row moved, none lost.
    assert_eq!(
        snapshot(&reader),
        [
            (
                "archived_revisions".to_owned(),
                "RES-0000-0470_v2".to_owned()
            ),
            ("category_res".to_owned(), "RES-0000-0100_v1".to_owned()),
            ("category_res".to_owned(), "RES-0000-0470_v1".to_owned()),
        ]
    );
}

#[test]
fn reads_during_many_updates_always_see_whole_states() {
    let folder = test_folder("safety-many");
    let mut fixture = resistors();
    {
        let mut blobs = fixture.blobs();
        update(&folder, &fixture.catalog, &mut blobs).unwrap();
    }
    let path = folder.join(DATABASE_FILE);
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reading = {
        let done = done.clone();
        std::thread::spawn(move || {
            let reader =
                Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            let mut reads = 0;
            while !done.load(std::sync::atomic::Ordering::SeqCst) || reads < 10 {
                let rows = snapshot(&reader);
                // Revision 2 is either current or archived, and the part's
                // other revision is in the other place: three rows always.
                assert_eq!(rows.len(), 3, "{rows:?}");
                let current: Vec<&str> = rows
                    .iter()
                    .filter(|(table, _)| table == "category_res")
                    .map(|(_, entry)| entry.as_str())
                    .collect();
                assert!(
                    current == ["RES-0000-0100_v1", "RES-0000-0470_v2"]
                        || current == ["RES-0000-0100_v1", "RES-0000-0470_v1"],
                    "{rows:?}"
                );
                reads += 1;
            }
            reads
        })
    };
    for round in 0..20 {
        fixture.archive("RES-0000-0470", 2, round % 2 == 0);
        let mut blobs = fixture.blobs();
        update(&folder, &fixture.catalog, &mut blobs).unwrap();
    }
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(reading.join().unwrap() >= 10);
}
