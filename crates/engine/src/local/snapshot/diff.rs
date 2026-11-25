use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use rfsync_fs::{FsEntry, FsEntryKind};
use rfsync_storage::{File, Folder};

use crate::error::EngineError;

#[derive(Debug)]
pub struct SnapshotDiff<'a> {
    pub added_folders: Vec<PathBuf>,

    pub added_files: Vec<&'a FsEntry>,
    pub modified_files: HashMap<i64, &'a FsEntry>,
    /// 스캔에서 사라진 파일. 이미 삭제 표시된 파일은 포함하지 않는다.
    pub removed_files: HashMap<i64, PathBuf>,
}

impl<'a> SnapshotDiff<'a> {
    pub fn new() -> Self {
        Self {
            added_folders: Vec::new(),
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
    // 사라진 폴더의 행은 지우지 않는다. 지우면 CASCADE로 아래 파일의 삭제 기록도 함께 사라진다.
    let stored_paths: HashSet<PathBuf> = stored
        .iter()
        .filter(|f| !f.relative_path.is_empty())
        .map(|f| PathBuf::from(&f.relative_path))
        .collect();

    for entry in scans.iter().filter(|e| e.is_dir()) {
        if !stored_paths.contains(&entry.path) {
            diff.added_folders.push(entry.path.clone());
        }
    }
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

                    // 삭제 표시된 파일이 다시 나타났으면 내용이 같아도 수정으로 보아 되살린다.
                    if file.deleted
                        || size != file.size
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
        if !file.deleted {
            diff.removed_files.insert(file.id, path);
        }
    }

    Ok(())
}
