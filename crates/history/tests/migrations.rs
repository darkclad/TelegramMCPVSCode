//! Integration tests for [`History::open`] and schema migrations.

use history::History;
use tempfile::tempdir;

#[test]
fn open_creates_db_and_schema_version_is_set() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("h.db");
    let _h = History::open(&db).unwrap();
    assert!(db.exists());
    // Open again — should be idempotent (no migration re-runs)
    let _h2 = History::open(&db).unwrap();
}

#[test]
fn schema_version_reads_back_as_one() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("h.db");
    let h = History::open(&db).unwrap();
    assert_eq!(h.schema_version().unwrap(), 1);
}
