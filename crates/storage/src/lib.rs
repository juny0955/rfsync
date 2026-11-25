use std::path::Path;

use crate::error::StorageError;

mod devices;
pub mod error;
mod file;
mod folder;
mod root;
mod sync_pending;

pub use devices::{Device, list_devices, upsert_device};
pub use file::{
    File, delete_file_by_id, get_file_by_folder_and_name, get_file_by_folder_id_and_name,
    get_files_by_root_id, get_files_in_folder_ids, insert_file, mark_file_deleted, update_file,
};
pub use folder::{
    Folder, delete_folder_by_id, get_folder_by_path, get_folders_by_root_id,
    get_folders_with_under_path, insert_folder,
};
pub use root::{Root, get_root, get_root_by_path, insert_root, list_roots, mark_root_has_snapshot};
pub use sync_pending::{SyncPending, delete_sync_pending, put_sync_pending, take_sync_pending};

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
