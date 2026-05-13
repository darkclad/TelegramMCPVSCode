//! `SQLite` schema definition and migration runner.

use crate::HistoryError;
use rusqlite::Connection;

#[allow(dead_code)] // wired up in Task 5
pub(crate) const CURRENT_VERSION: u32 = 1;

#[allow(dead_code)] // wired up in Task 5
pub(crate) fn migrate(_conn: &mut Connection) -> Result<u32, HistoryError> {
    unimplemented!("filled by Task 5")
}
