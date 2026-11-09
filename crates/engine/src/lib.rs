use std::{
    collections::HashMap,
    error::Error,
    io,
    path::{Component, Path, PathBuf},
};

use rfsync_fs::EntryKind;

pub fn register_root(root: &Path, db_path: &Path) -> Result<i64, Box<dyn Error>> {
    let conn = rfsync_storage::open(db_path)?;

    let absolute_root = root.canonicalize()?;
    let local_path = absolute_root.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "root를 문자열로 표현할 수 없습니다",
        )
    })?;

    let root_id = rfsync_storage::insert_root(&conn, local_path)?;
    Ok(root_id)
}

pub fn create_initial_snapshot(root_id: i64, db_path: &Path) -> Result<(), Box<dyn Error>> {
    let mut conn = rfsync_storage::open(db_path)?;

    let root = rfsync_storage::get_root(&conn, root_id)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "등록되지 않은 root 입니다"))?;

    if root.has_snapshot {
        return Err(
            io::Error::new(io::ErrorKind::AlreadyExists, "이미 스냅샵이 존재합니다").into(),
        );
    }

    let result = rfsync_fs::scan(Path::new(&root.local_path))?;

    let mut folder_ids = HashMap::<PathBuf, i64>::new();

    // === START TRANSACTION ===
    let tx = conn.transaction()?;
    let root_folder_id = rfsync_storage::insert_folder(&tx, root_id, "")?;
    folder_ids.insert(PathBuf::new(), root_folder_id);

    for entry in &result.entries {
        if entry.is_dir() {
            let relative_path = portable_path(&entry.path)?;
            let folder_id = rfsync_storage::insert_folder(&tx, root_id, &relative_path)?;
            folder_ids.insert(entry.path.clone(), folder_id);
        }
    }

    for entry in &result.entries {
        if let EntryKind::File {
            size,
            hash,
            modified,
        } = &entry.kind
        {
            let parent = entry.path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "부모 폴더가 없습니다")
            })?;

            let folder_id = *folder_ids.get(parent).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "부모 폴더 ID가 없습니다")
            })?;

            let relative_path = portable_path(&entry.path)?;
            let name = relative_path.rsplit('/').next().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "파일 이름이 없습니다")
            })?;

            let modified = modified.duration_since(std::time::UNIX_EPOCH)?;
            let file = rfsync_storage::File {
                id: 0,
                folder_id,
                name: name.to_owned(),
                size: i64::try_from(*size)?,
                hash: hash.clone(),
                modified_secs: i64::try_from(modified.as_secs())?,
                modified_nanos: modified.subsec_nanos(),
            };

            rfsync_storage::insert_file(&tx, &file)?;
        }
    }
    rfsync_storage::mark_root_has_snapshot(&tx, root_id)?;
    tx.commit()?;
    // === END TRANSACTION ===

    Ok(())
}

fn portable_path(path: &Path) -> io::Result<String> {
    let mut parts = Vec::new();

    for compoenct in path.components() {
        let Component::Normal(name) = compoenct else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "상대 경로의 일반 이름만 허용됩니다",
            ));
        };

        let name = name.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "경로를 UTF-8로 표현할 수 없습니다",
            )
        })?;

        if name.contains('\\') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "경로애 지원하지 않는 문자가 있습니다",
            ));
        }

        parts.push(name);
    }

    if parts.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "빈 항목 경로는 허용하지 않습니다",
        ));
    }

    Ok(parts.join("/"))
}
