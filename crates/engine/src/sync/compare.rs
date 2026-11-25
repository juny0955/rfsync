use std::collections::HashMap;

use rfsync_protocol::Entry;

/// 상대의 파일 목록을 내 목록과 비교한 결과. 담긴 항목은 모두 상대 쪽 것이다.
#[derive(Debug, Default, PartialEq)]
pub(super) struct IndexComparison {
    /// 상대에게만 있는 파일. 내가 받아올 후보다.
    pub missing: Vec<Entry>,
    /// 양쪽에 있지만 내용(해시)이 다른 파일. 누가 최신인지 알 근거가 아직 없어 건드리지 않는다.
    pub changed: Vec<Entry>,
}

/// 경로를 기준으로 상대 목록을 내 목록과 비교한다. 결과는 경로순이다.
///
/// - 내게만 있는 파일은 무시한다. 삭제인지 새로 만든 것인지 구분할 정보(삭제 기록, 버전)가 없고,
///   상대가 같은 비교를 해서 자기에게 없는 파일을 요청하기 때문이다.
/// - 내용이 같으면 수정시각이 달라도 같은 파일로 본다. 전송할 필요가 없다.
pub(super) fn compare_index(local: &[Entry], remote: &[Entry]) -> IndexComparison {
    let local_hashes: HashMap<&str, &[u8; 32]> = local
        .iter()
        .map(|entry| (entry.path.as_str(), &entry.hash))
        .collect();

    let mut comparison = IndexComparison::default();
    for entry in remote {
        match local_hashes.get(entry.path.as_str()) {
            None => comparison.missing.push(entry.clone()),
            Some(hash) if **hash != entry.hash => comparison.changed.push(entry.clone()),
            Some(_) => {}
        }
    }

    comparison.missing.sort_by(|a, b| a.path.cmp(&b.path));
    comparison.changed.sort_by(|a, b| a.path.cmp(&b.path));
    comparison
}

/// 내 목록이 `before`에서 `after`로 바뀌었을 때, 상대에게 알릴 항목을 골라낸다. 결과는 `after`의 순서를 따른다.
///
/// 새로 생겼거나 내용(해시)이 바뀐 파일만 담고, 담기는 것은 `after`의 항목이다.
/// 삭제는 담지 않는다. 삭제를 표현할 방법(삭제 기록)이 아직 없다.
pub(super) fn changed_entries(before: &[Entry], after: &[Entry]) -> Vec<Entry> {
    let before_hashes: HashMap<&str, &[u8; 32]> = before
        .iter()
        .map(|entry| (entry.path.as_str(), &entry.hash))
        .collect();

    after
        .iter()
        .filter(|entry| before_hashes.get(entry.path.as_str()) != Some(&&entry.hash))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, hash: u8) -> Entry {
        Entry {
            path: path.to_owned(),
            size: 1,
            hash: [hash; 32],
            modified_secs: 100,
            modified_nanos: 0,
        }
    }

    fn paths(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.path.as_str()).collect()
    }

    #[test]
    fn 두_목록이_같으면_비교_결과가_비어_있다() {
        let list = vec![entry("a.txt", 1), entry("sub/b.txt", 2)];

        let comparison = compare_index(&list, &list);

        assert_eq!(comparison, IndexComparison::default());
    }

    #[test]
    fn 상대에게만_있는_파일은_missing이다() {
        let local = vec![entry("a.txt", 1)];
        let remote = vec![entry("a.txt", 1), entry("new.txt", 2)];

        let comparison = compare_index(&local, &remote);

        assert_eq!(comparison.missing, vec![entry("new.txt", 2)]);
        assert!(comparison.changed.is_empty());
    }

    #[test]
    fn 내게만_있는_파일은_무시한다() {
        let local = vec![entry("a.txt", 1), entry("mine.txt", 2)];
        let remote = vec![entry("a.txt", 1)];

        let comparison = compare_index(&local, &remote);

        assert_eq!(comparison, IndexComparison::default());
    }

    #[test]
    fn 같은_경로에서_해시가_다르면_changed이고_상대_쪽_항목을_담는다() {
        let local = vec![entry("a.txt", 1)];
        let remote = vec![entry("a.txt", 9)];

        let comparison = compare_index(&local, &remote);

        assert_eq!(comparison.changed, vec![entry("a.txt", 9)]);
        assert!(comparison.missing.is_empty());
    }

    #[test]
    fn 내용이_같으면_수정시각이_달라도_같은_파일이다() {
        let local = vec![entry("a.txt", 1)];
        let mut remote_entry = entry("a.txt", 1);
        remote_entry.modified_secs = 999;
        remote_entry.modified_nanos = 7;

        let comparison = compare_index(&local, &[remote_entry]);

        assert_eq!(comparison, IndexComparison::default());
    }

    #[test]
    fn 경로가_같아도_폴더가_다르면_다른_파일이다() {
        let local = vec![entry("a.txt", 1)];
        let remote = vec![entry("sub/a.txt", 1)];

        let comparison = compare_index(&local, &remote);

        assert_eq!(paths(&comparison.missing), ["sub/a.txt"]);
    }

    #[test]
    fn 내_목록이_비어_있으면_상대의_모든_파일이_missing이다() {
        let remote = vec![entry("b.txt", 2), entry("a.txt", 1)];

        let comparison = compare_index(&[], &remote);

        assert_eq!(paths(&comparison.missing), ["a.txt", "b.txt"]);
        assert!(comparison.changed.is_empty());
    }

    #[test]
    fn 상대_목록이_비어_있으면_비교_결과가_비어_있다() {
        let local = vec![entry("a.txt", 1)];

        assert_eq!(compare_index(&local, &[]), IndexComparison::default());
        assert_eq!(compare_index(&[], &[]), IndexComparison::default());
    }

    #[test]
    fn 결과는_입력_순서와_상관없이_경로순이다() {
        let local = vec![entry("x.txt", 1), entry("y.txt", 1)];
        let remote = vec![
            entry("y.txt", 2),
            entry("c.txt", 3),
            entry("x.txt", 2),
            entry("a.txt", 3),
        ];

        let comparison = compare_index(&local, &remote);

        assert_eq!(paths(&comparison.missing), ["a.txt", "c.txt"]);
        assert_eq!(paths(&comparison.changed), ["x.txt", "y.txt"]);
    }

    #[test]
    fn 없는_파일과_다른_파일이_섞여_있으면_각각_나뉜다() {
        let local = vec![entry("same.txt", 1), entry("diff.txt", 1)];
        let remote = vec![
            entry("same.txt", 1),
            entry("diff.txt", 2),
            entry("new.txt", 3),
        ];

        let comparison = compare_index(&local, &remote);

        assert_eq!(paths(&comparison.missing), ["new.txt"]);
        assert_eq!(paths(&comparison.changed), ["diff.txt"]);
    }

    #[test]
    fn 새로_생긴_파일과_내용이_바뀐_파일만_바뀐_항목이다() {
        let before = vec![entry("a.txt", 1), entry("b.txt", 2)];
        let after = vec![entry("a.txt", 1), entry("b.txt", 9), entry("c.txt", 3)];

        let changed = changed_entries(&before, &after);

        assert_eq!(paths(&changed), ["b.txt", "c.txt"]);
        assert_eq!(changed[0].hash, [9; 32], "새 목록의 항목을 돌려준다");
    }

    #[test]
    fn 내용이_같으면_수정시각이_달라도_바뀐_항목이_아니다() {
        let before = vec![entry("a.txt", 1)];
        let mut touched = entry("a.txt", 1);
        touched.modified_secs = 999;

        assert!(changed_entries(&before, &[touched]).is_empty());
    }

    #[test]
    fn 삭제된_파일은_바뀐_항목에_넣지_않는다() {
        let before = vec![entry("a.txt", 1), entry("gone.txt", 2)];
        let after = vec![entry("a.txt", 1)];

        assert!(changed_entries(&before, &after).is_empty());
    }
}
