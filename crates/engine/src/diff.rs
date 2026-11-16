use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use rfsync_fs::{FsEntry, FsEntryKind};
use rfsync_storage::{File, Folder};

use crate::error::EngineError;

#[derive(Debug)]
pub struct SnapshotDiff<'a> {
    pub added_folders: Vec<PathBuf>,
    pub removed_folders: HashMap<i64, PathBuf>,

    pub added_files: Vec<&'a FsEntry>,
    pub modified_files: HashMap<i64, &'a FsEntry>,
    pub removed_files: HashMap<i64, PathBuf>, // removed_folder 하위 파일은 포함하지 않는다
}

impl<'a> SnapshotDiff<'a> {
    pub fn new() -> Self {
        Self {
            added_folders: Vec::new(),
            removed_folders: HashMap::new(),
            added_files: Vec::new(),
            modified_files: HashMap::new(),
            removed_files: HashMap::new(),
        }
    }
}

impl Default for SnapshotDiff<'_> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn calc_diff<'a>(
    scans: &'a [FsEntry],
    folders: &[Folder],
    files: &[File],
) -> Result<SnapshotDiff<'a>, EngineError> {
    let mut diff = SnapshotDiff::default();

    diff_folder(&mut diff, scans, folders);
    diff_file(&mut diff, scans, files, folders)?;

    Ok(diff)
}

fn diff_folder(diff: &mut SnapshotDiff, scans: &[FsEntry], stored: &[Folder]) {
    let stored_folders: HashMap<PathBuf, i64> = stored
        .iter()
        .filter(|f| !f.relative_path.is_empty())
        .map(|f| (PathBuf::from(&f.relative_path), f.id))
        .collect();

    let mut remaining = stored_folders;

    for entry in scans.iter().filter(|e| e.is_dir()) {
        if remaining.remove(&entry.path).is_none() {
            diff.added_folders.push(entry.path.clone());
        }
    }

    diff.removed_folders
        .extend(remaining.into_iter().map(|(path, id)| (id, path)));
}

fn diff_file<'a>(
    diff: &mut SnapshotDiff<'a>,
    scans: &'a [FsEntry],
    stored: &[File],
    folders: &[Folder],
) -> Result<(), EngineError> {
    let folder_paths: HashMap<i64, &str> = folders
        .iter()
        .map(|f| (f.id, f.relative_path.as_str()))
        .collect();

    let mut remaining: HashMap<PathBuf, &File> = HashMap::new();

    for file in stored {
        let parent = folder_paths.get(&file.folder_id).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("부모 폴더를 찾을 수 없습니다: {}", file.folder_id),
            )
        })?;

        let path = Path::new(parent).join(&file.name);
        remaining.insert(path, file);
    }

    for entry in scans.iter().filter(|e| !e.is_dir()) {
        match remaining.remove(&entry.path) {
            None => {
                diff.added_files.push(entry);
            }
            Some(file) => {
                if let FsEntryKind::File {
                    size,
                    hash,
                    modified,
                } = &entry.kind
                {
                    let timestamp = modified.duration_since(std::time::UNIX_EPOCH)?;
                    let size = i64::try_from(*size)?;
                    let secs = i64::try_from(timestamp.as_secs())?;

                    if size != file.size
                        || hash != &file.hash
                        || secs != file.modified_secs
                        || timestamp.subsec_nanos() != file.modified_nanos
                    {
                        diff.modified_files.insert(file.id, entry);
                    }
                }
            }
        }
    }

    for (path, file) in remaining {
        // 삭제된 폴더 하위 파일은 CASCADE로 삭제
        if diff
            .removed_folders
            .values()
            .any(|folder| path.starts_with(folder))
        {
            continue;
        }

        diff.removed_files.insert(file.id, path);
    }

    Ok(())
}
