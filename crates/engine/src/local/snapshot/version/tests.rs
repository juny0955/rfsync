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

fn stored(hash_char: &str, deleted: bool, version: &VersionVector) -> File {
    File {
        id: 1,
        folder_id: 1,
        name: "a.txt".into(),
        size: 1,
        hash: hash_char.repeat(64),
        modified_secs: 0,
        modified_nanos: 0,
        deleted,
        version: version.to_bytes().unwrap(),
    }
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

#[test]
fn 이전_행이_없는_새_파일은_내_카운터가_1이다() {
    let bytes = next_file_version(None, &"a".repeat(64), None, device("me")).unwrap();

    assert_eq!(
        VersionVector::from_bytes(&bytes).unwrap(),
        vector(&[("me", 1)])
    );
}

#[test]
fn 해시가_같으면_version_바이트를_그대로_둔다() {
    let version = vector(&[("peer", 2), ("me", 1)]);
    let file = stored("a", false, &version);

    let bytes = next_file_version(Some(&file), &"a".repeat(64), None, device("me")).unwrap();

    assert_eq!(bytes, file.version);
}

#[test]
fn 해시가_달라지면_이전_version에서_내_카운터를_올린다() {
    let file = stored("a", false, &vector(&[("peer", 2), ("me", 1)]));

    let bytes = next_file_version(Some(&file), &"b".repeat(64), None, device("me")).unwrap();

    assert_eq!(
        VersionVector::from_bytes(&bytes).unwrap(),
        vector(&[("peer", 2), ("me", 2)])
    );
}

#[test]
fn 삭제_표시된_파일이_같은_내용으로_돌아오면_올린다() {
    let file = stored("a", true, &vector(&[("me", 2)]));

    let bytes = next_file_version(Some(&file), &"a".repeat(64), None, device("me")).unwrap();

    assert_eq!(
        VersionVector::from_bytes(&bytes).unwrap(),
        vector(&[("me", 3)])
    );
}

#[test]
fn 이전_version을_읽을_수_없으면_에러다() {
    let mut file = stored("a", false, &VersionVector::default());
    file.version = Vec::new();

    assert!(next_file_version(Some(&file), &"b".repeat(64), None, device("me")).is_err());
}

fn received(hash_char: &str, deleted: bool, version: &VersionVector) -> SyncPending {
    SyncPending {
        path: "a.txt".into(),
        hash: hash_char.repeat(64),
        deleted,
        version: version.to_bytes().unwrap(),
    }
}

#[test]
fn 받은_새_파일은_상대의_version을_그대로_쓴다() {
    let theirs = vector(&[("peer", 3)]);
    let pending = received("a", false, &theirs);

    let bytes = next_file_version(None, &"a".repeat(64), Some(&pending), device("me")).unwrap();

    assert_eq!(VersionVector::from_bytes(&bytes).unwrap(), theirs);
}

#[test]
fn 받아서_덮어쓴_파일도_상대의_version을_그대로_쓴다() {
    let file = stored("a", false, &vector(&[("peer", 1), ("me", 1)]));
    let theirs = vector(&[("peer", 2), ("me", 1)]);
    let pending = received("b", false, &theirs);

    let bytes =
        next_file_version(Some(&file), &"b".repeat(64), Some(&pending), device("me")).unwrap();

    assert_eq!(VersionVector::from_bytes(&bytes).unwrap(), theirs);
}

#[test]
fn 받은_기록과_내용이_다르면_받은_이력을_합친_뒤_올린다() {
    let file = stored("a", false, &vector(&[("me", 1)]));
    let pending = received("b", false, &vector(&[("peer", 2)]));

    let bytes =
        next_file_version(Some(&file), &"c".repeat(64), Some(&pending), device("me")).unwrap();

    assert_eq!(
        VersionVector::from_bytes(&bytes).unwrap(),
        vector(&[("peer", 2), ("me", 2)])
    );
}

#[test]
fn 상대가_지워서_지운_파일은_상대의_version을_그대로_쓴다() {
    let file = stored("a", false, &vector(&[("me", 1)]));
    let theirs = vector(&[("me", 1), ("peer", 1)]);
    let pending = received("a", true, &theirs);

    let bytes = deleted_file_version(&file, Some(&pending), device("me")).unwrap();

    assert_eq!(VersionVector::from_bytes(&bytes).unwrap(), theirs);
}

#[test]
fn 받은_파일을_지운_기록이_아니면_내가_지운_것이다() {
    let file = stored("a", false, &vector(&[("me", 1)]));
    // 파일을 받는다는 기록이 남아 있어도 지우기는 내가 한 것이다.
    let pending = received("a", false, &vector(&[("peer", 1)]));

    let bytes = deleted_file_version(&file, Some(&pending), device("me")).unwrap();

    assert_eq!(
        VersionVector::from_bytes(&bytes).unwrap(),
        vector(&[("peer", 1), ("me", 2)])
    );
}

#[test]
fn 삭제하면_이전_version에서_내_카운터만_올린다() {
    let file = stored("a", false, &vector(&[("peer", 2), ("me", 1)]));

    let bytes = deleted_file_version(&file, None, device("me")).unwrap();

    assert_eq!(
        VersionVector::from_bytes(&bytes).unwrap(),
        vector(&[("peer", 2), ("me", 2)])
    );
}

#[test]
fn 삭제할_파일의_version을_읽을_수_없으면_에러다() {
    let mut file = stored("a", false, &VersionVector::default());
    file.version = Vec::new();

    assert!(deleted_file_version(&file, None, device("me")).is_err());
}
