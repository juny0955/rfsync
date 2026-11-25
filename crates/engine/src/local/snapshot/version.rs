use rfsync_protocol::{DeviceId, VersionVector};
use rfsync_storage::{File, SyncPending};

use crate::error::EngineError;

/// 파일이 지워진 것을 DB에 반영할 때 저장할 version 바이트를 정한다.
/// 삭제도 한 번의 변경이므로 이전 version에서 내 카운터를 올린다. 해시는 지우기 직전 내용의 것이다.
/// 상대가 지우라고 해서 지운 것이면(`pending`이 삭제와 같다) 상대의 version을 그대로 쓴다.
pub(super) fn deleted_file_version(
    file: &File,
    pending: Option<&SyncPending>,
    me: DeviceId,
) -> Result<Vec<u8>, EngineError> {
    let before = VersionVector::from_bytes(&file.version)?;
    Ok(next_version(&before, &file.hash, true, pending, me).to_bytes()?)
}

/// 디스크에서 본 파일을 DB에 반영할 때 저장할 version 바이트를 정한다.
///
/// - 상대에게서 받아 적용한 파일이면(`pending`이 관측한 내용과 같다) 상대의 version을 그대로 쓴다.
/// - 이전 행이 없으면 새 파일이므로 `{나: 1}`이다.
/// - 삭제 표시가 아니고 해시가 같으면 내용이 안 바뀐 것(수정 시각만 달라진 경우)이라 version을 그대로 둔다.
/// - 해시가 달라졌거나, 삭제 표시된 파일이 돌아왔으면 이전 version에서 내 카운터를 올린다.
pub(super) fn next_file_version(
    previous: Option<&File>,
    observed_hash: &str,
    pending: Option<&SyncPending>,
    me: DeviceId,
) -> Result<Vec<u8>, EngineError> {
    let unchanged = previous.is_some_and(|file| !file.deleted && file.hash == observed_hash);
    let received = pending.is_some_and(|p| p.hash == observed_hash && !p.deleted);
    if let Some(file) = previous
        && unchanged
        && !received
    {
        return Ok(file.version.clone());
    }

    let before = match previous {
        Some(file) => VersionVector::from_bytes(&file.version)?,
        None => VersionVector::default(),
    };
    Ok(next_version(&before, observed_hash, false, pending, me).to_bytes()?)
}

/// 감시자가 파일의 변경을 DB에 반영할 때 저장할 버전을 정한다.
///
/// - 상대에게서 받아 적용한 변경이면(`pending`이 관측한 상태와 같다) 상대의 버전을 그대로 쓴다.
///   내 카운터를 올리지 않아야 상대의 이력이 사라지지 않는다.
/// - 그 밖에는 내가 직접 고친 것이다. 이전 버전에서 내 카운터만 1 올린다.
///   받은 변경 위에 내가 또 고친 경우(`pending`은 있지만 상태가 다르다)에는 받은 이력도 합친 뒤 올린다.
///
/// `observed_hash`는 디스크에서 관측한 해시다. 삭제를 관측했을 때는 지우기 직전 내용의 해시를 넘긴다.
pub(super) fn next_version(
    previous: &VersionVector,
    observed_hash: &str,
    observed_deleted: bool,
    pending: Option<&SyncPending>,
    me: DeviceId,
) -> VersionVector {
    if let Some(pending) = pending
        && pending.hash == observed_hash
        && pending.deleted == observed_deleted
        && let Ok(version) = VersionVector::from_bytes(&pending.version)
    {
        return version;
    }

    let mut next = previous.clone();
    if let Some(pending) = pending
        && let Ok(version) = VersionVector::from_bytes(&pending.version)
    {
        next.merge(&version);
    }
    next.bump(me);
    next
}

#[cfg(test)]
mod tests;
