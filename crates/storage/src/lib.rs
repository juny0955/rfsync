use std::path::Path;

use crate::error::StorageError;

mod devices;
pub mod error;
mod file;
mod folder;
mod root;
mod sync_pending;

pub use devices::{Device, list_devices, upsert_device};
pub use file::*;
pub use folder::*;
pub use root::*;
pub use sync_pending::*;

pub use rusqlite::{Connection as DbConnection, Transaction, TransactionBehavior};

pub fn open(path: &Path) -> Result<DbConnection, StorageError> {
    let mut conn = DbConnection::open(path)?;

    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    initialize_schema(&mut conn)?;

    Ok(conn)
}

fn initialize_schema(conn: &mut DbConnection) -> Result<(), StorageError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    match version {
        0 => {
            let has_objects: bool = tx.query_row(
                "SELECT EXISTS (
                        SELECT 1 FROM sqlite_schema
                        WHERE name NOT LIKE 'sqlite_%'
                )",
                [],
                |row| row.get(0),
            )?;

            if has_objects {
                return Err(StorageError::UnversionedDatabase);
            }

            tx.execute_batch(include_str!("../sql/schema.sql"))?;
            tx.execute_batch("PRAGMA user_version = 1;")?;
        }
        1 => {}
        _ => {
            return Err(StorageError::UnsupportedSchemaVersion(version));
        }
    }

    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests;
