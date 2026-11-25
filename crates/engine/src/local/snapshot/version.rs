use rfsync_protocol::{DeviceId, VersionVector};
use rfsync_storage::SyncPending;

/// 감시자가 파일의 변경을 DB에 반영할 때 저장할 버전을 정한다.
///
/// - 상대에게서 받아 적용한 변경이면(`pending`이 관측한 상태와 같다) 상대의 버전을 그대로 쓴다.
///   내 카운터를 올리지 않아야 상대의 이력이 사라지지 않는다.
/// - 그 밖에는 내가 직접 고친 것이다. 이전 버전에서 내 카운터만 1 올린다.
///   받은 변경 위에 내가 또 고친 경우(`pending`은 있지만 상태가 다르다)에는 받은 이력도 합친 뒤 올린다.
///
/// `observed_hash`는 디스크에서 관측한 해시다. 삭제를 관측했을 때는 지우기 직전 내용의 해시를 넘긴다.
#[allow(dead_code)] // 4-3에서 감시자 반영 경로에 연결한다.
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
mod tests {
    use super::*;
    use rfsync_protocol::Causality;

    fn device(name: &str) -> DeviceId {
        DeviceId::from_cert(name.as_bytes())
    }

    /// `[("a", 2), ("b", 1)]`처럼 장치 이름과 횟수로 벡터를 만든다.
    fn vector(counts: &[(&str, u64)]) -> VersionVector {
        let mut vector = VersionVector::default();
        for &(name, count) in counts {
            for _ in 0..count {
                vector.bump(device(name));
            }
        }
        vector
    }

    fn pending(hash_char: &str, deleted: bool, version: &VersionVector) -> SyncPending {
        SyncPending {
            path: "a.txt".into(),
            hash: hash_char.repeat(64),
            deleted,
            version: version.to_bytes().unwrap(),
        }
    }

    #[test]
    fn 받은_기록이_없으면_내_카운터만_올린다() {
        let previous = vector(&[("me", 1), ("peer", 2)]);

        let next = next_version(&previous, &"a".repeat(64), false, None, device("me"));

        assert_eq!(next, vector(&[("me", 2), ("peer", 2)]));
    }

    #[test]
    fn 처음_보는_파일은_내_카운터가_1이_된다() {
        let next = next_version(
            &VersionVector::default(),
            &"a".repeat(64),
            false,
            None,
            device("me"),
        );

        assert_eq!(next, vector(&[("me", 1)]));
    }

    #[test]
    fn 받은_변경과_상태가_같으면_상대의_버전을_그대로_쓴다() {
        let previous = vector(&[("me", 1)]);
        let received = vector(&[("peer", 3)]);
        let pending = pending("a", false, &received);

        let next = next_version(
            &previous,
            &"a".repeat(64),
            false,
            Some(&pending),
            device("me"),
        );

        assert_eq!(next, received);
    }

    #[test]
    fn 받은_삭제와_상태가_같으면_상대의_버전을_그대로_쓴다() {
        let previous = vector(&[("me", 1)]);
        let received = vector(&[("me", 1), ("peer", 1)]);
        let pending = pending("a", true, &received);

        let next = next_version(
            &previous,
            &"a".repeat(64),
            true,
            Some(&pending),
            device("me"),
        );

        assert_eq!(next, received);
    }

    #[test]
    fn 받은_뒤_내가_또_고쳤으면_받은_이력을_합치고_올린다() {
        let previous = vector(&[("me", 1)]);
        let received = vector(&[("me", 1), ("peer", 3)]);
        let pending = pending("a", false, &received);

        // 받은 해시는 a인데 디스크에서는 b가 관측됐다.
        let next = next_version(
            &previous,
            &"b".repeat(64),
            false,
            Some(&pending),
            device("me"),
        );

        assert_eq!(next, vector(&[("me", 2), ("peer", 3)]));
        assert_eq!(next.compare(&received), Causality::LocalAhead);
    }

    #[test]
    fn 받은_변경과_삭제_여부가_다르면_내가_고친_것이다() {
        let previous = VersionVector::default();
        let received = vector(&[("peer", 1)]);
        // 상대의 파일을 받았는데 곧바로 내가 지웠다. 해시는 같고 삭제 여부만 다르다.
        let pending = pending("a", false, &received);

        let next = next_version(
            &previous,
            &"a".repeat(64),
            true,
            Some(&pending),
            device("me"),
        );

        assert_eq!(next, vector(&[("me", 1), ("peer", 1)]));
    }

    #[test]
    fn 읽을_수_없는_받은_버전은_무시하고_직접_고친_것으로_본다() {
        let previous = vector(&[("peer", 1)]);
        let broken = SyncPending {
            path: "a.txt".into(),
            hash: "a".repeat(64),
            deleted: false,
            version: Vec::new(),
        };

        let next = next_version(
            &previous,
            &"a".repeat(64),
            false,
            Some(&broken),
            device("me"),
        );

        assert_eq!(next, vector(&[("me", 1), ("peer", 1)]));
    }
}
