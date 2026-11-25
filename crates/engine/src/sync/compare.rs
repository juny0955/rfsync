use std::collections::HashMap;

use rfsync_protocol::{Causality, Entry};

/// 상대의 파일 목록을 내 목록과 비교한 결과. 담긴 항목은 모두 상대 쪽 것이다.
#[derive(Debug, Default, PartialEq)]
pub(super) struct IndexComparison {
    /// 받아 올 파일. 상대에게만 있거나, 내가 지운 뒤에 상대가 새로 만들거나 고친 파일이다.
    pub missing: Vec<Entry>,
    /// 양쪽에 있고 내용이 다르며 상대의 버전이 내 것을 앞선 파일. 상대가 더 최신이라 덮어써야 한다.
    pub updated: Vec<Entry>,
    /// 양쪽에 있고 내용이 다르며 버전이 서로 앞서지 않는(`Concurrent`) 파일. 동시에 고친 충돌이다.
    /// 누가 이기는지는 여기서 정하지 않는다(`conflict::decide_winner`).
    pub conflicts: Vec<Entry>,
    /// 상대가 지웠고 상대의 버전이 내 것을 앞서는, 내게 남아 있는 파일. 내가 지울 후보다.
    pub deleted: Vec<Entry>,
    /// 내용과 삭제 여부는 같은데 버전이 다른 파일. 두 버전을 합쳐 저장하면 서로 앞서지 않는 상태가 풀린다.
    /// 같은 내용을 각자 따로 등록한 경우가 대표적이다.
    pub merge: Vec<Entry>,
}

/// 경로를 기준으로 상대 목록을 내 목록과 비교한다. 결과는 경로순이다.
///
/// 양쪽 모두 삭제 기록(`deleted`)이 들어 있다. 삭제 기록의 해시는 지우기 직전 내용의 해시다.
/// 내용이 같으면 수정시각이 달라도 같은 파일로 본다. 내용이 다를 때 누가 최신인지는 버전 벡터로 정한다.
///
/// | 상대 | 나 | 버전 | 결과 |
/// |---|---|---|---|
/// | 파일 | 없음 | | `missing` |
/// | 삭제 | 없음 | | 무시 |
/// | 같은 내용·같은 삭제 여부 | | 다름 | `merge` |
/// | 같은 내용·같은 삭제 여부 | | 같음 | 무시 |
/// | 파일 | 파일, 해시 다름 | 상대가 앞섬 | `updated` |
/// | 파일 | 파일, 해시 다름 | 동시 | `conflicts` |
/// | 파일 | 파일, 해시 다름 | 내가 앞서거나 같음 | 무시 |
/// | 파일 | 삭제 기록 | 상대가 앞섬 | `missing` (같은 내용으로 되살린 것도 포함) |
/// | 파일 | 삭제 기록, 해시 다름 | 동시 | `missing` (수정이 삭제를 이긴다) |
/// | 파일 | 삭제 기록 | 그 밖 | 무시 (내가 지운 옛 내용이므로 되살리지 않는다) |
/// | 삭제 | 파일 | 상대가 앞섬 | `deleted` |
/// | 삭제 | 파일 | 그 밖 | 무시 (내가 고쳤거나 동시이면 수정이 삭제를 이긴다) |
/// | 삭제 | 삭제 기록, 해시 다름 | | 무시 |
pub(super) fn compare_index(local: &[Entry], remote: &[Entry]) -> IndexComparison {
    let local_by_path: HashMap<&str, &Entry> = local
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();

    let mut comparison = IndexComparison::default();
    for entry in remote {
        let Some(mine) = local_by_path.get(entry.path.as_str()) else {
            if !entry.deleted {
                comparison.missing.push(entry.clone());
            }
            continue;
        };

        let causality = mine.version.compare(&entry.version);
        let same_content = mine.hash == entry.hash;
        let remote_ahead = causality == Causality::RemoteAhead;
        let concurrent = causality == Causality::Concurrent;

        if same_content && mine.deleted == entry.deleted {
            if causality != Causality::Equal {
                comparison.merge.push(entry.clone());
            }
            continue;
        }

        match (entry.deleted, mine.deleted) {
            (false, false) if !same_content && remote_ahead => {
                comparison.updated.push(entry.clone())
            }
            (false, false) if !same_content && concurrent => {
                comparison.conflicts.push(entry.clone())
            }
            (false, true) if remote_ahead || (concurrent && !same_content) => {
                comparison.missing.push(entry.clone())
            }
            (true, false) if remote_ahead => comparison.deleted.push(entry.clone()),
            _ => {}
        }
    }

    for list in [
        &mut comparison.missing,
        &mut comparison.updated,
        &mut comparison.conflicts,
        &mut comparison.deleted,
        &mut comparison.merge,
    ] {
        list.sort_by(|a, b| a.path.cmp(&b.path));
    }
    comparison
}

/// 내 목록이 `before`에서 `after`로 바뀌었을 때, 상대에게 알릴 항목을 골라낸다. 결과는 `after`의 순서를 따른다.
///
/// 새로 생겼거나, 내용(해시)이 바뀌었거나, 삭제 여부가 바뀐 항목만 담고, 담기는 것은 `after`의 항목이다.
/// 파일을 지우면 `after`에 삭제 기록(`deleted`)이 남으므로 삭제도 여기에 담긴다.
/// 같은 내용으로 다시 만든 경우도 삭제 여부가 바뀐 것이라 담긴다.
pub(super) fn changed_entries(before: &[Entry], after: &[Entry]) -> Vec<Entry> {
    let before_state: HashMap<&str, (&[u8; 32], bool)> = before
        .iter()
        .map(|entry| (entry.path.as_str(), (&entry.hash, entry.deleted)))
        .collect();

    after
        .iter()
        .filter(|entry| {
            before_state.get(entry.path.as_str()) != Some(&(&entry.hash, entry.deleted))
        })
        .cloned()
        .collect()
}

/// 응답 헤더의 크기와 해시가 상대가 보낸 목록의 항목과 같은가.
pub(super) fn matches_entry(entry: &Entry, size: u64, hash: &[u8; 32]) -> bool {
    entry.size == size && entry.hash == *hash
}

#[cfg(test)]
mod tests {
    use rfsync_protocol::{DeviceId, VersionVector};

    use super::*;

    fn entry(path: &str, hash: u8) -> Entry {
        Entry {
            path: path.to_owned(),
            size: 1,
            hash: [hash; 32],
            modified_secs: 100,
            modified_nanos: 0,
            deleted: false,
            version: VersionVector::default(),
        }
    }

    fn deleted(path: &str, hash: u8) -> Entry {
        Entry {
            deleted: true,
            ..entry(path, hash)
        }
    }

    fn paths(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.path.as_str()).collect()
    }

    /// 장치 이름 목록으로 만든 버전 벡터. 같은 이름이 여러 번이면 그만큼 올린 것이다.
    fn version(names: &[&str]) -> VersionVector {
        let mut version = VersionVector::default();
        for name in names {
            version.bump(DeviceId::from_cert(name.as_bytes()));
        }
        version
    }

    fn with(mut entry: Entry, names: &[&str]) -> Entry {
        entry.version = version(names);
        entry
    }

    #[test]
    fn 두_목록이_같으면_비교_결과가_비어_있다() {
        let list = vec![entry("a.txt", 1), entry("sub/b.txt", 2)];

        assert_eq!(compare_index(&list, &list), IndexComparison::default());
    }

    #[test]
    fn 상대에게만_있는_파일은_missing이다() {
        let local = vec![entry("a.txt", 1)];
        let remote = vec![entry("a.txt", 1), entry("new.txt", 2)];

        let comparison = compare_index(&local, &remote);

        assert_eq!(comparison.missing, vec![entry("new.txt", 2)]);
        assert!(comparison.updated.is_empty() && comparison.conflicts.is_empty());
    }

    #[test]
    fn 내게만_있는_파일은_무시한다() {
        let local = vec![entry("a.txt", 1), entry("mine.txt", 2)];
        let remote = vec![entry("a.txt", 1)];

        assert_eq!(compare_index(&local, &remote), IndexComparison::default());
    }

    #[test]
    fn 내용이_같으면_수정시각이_달라도_같은_파일이다() {
        let local = vec![entry("a.txt", 1)];
        let mut remote_entry = entry("a.txt", 1);
        remote_entry.modified_secs = 999;
        remote_entry.modified_nanos = 7;

        assert_eq!(
            compare_index(&local, &[remote_entry]),
            IndexComparison::default()
        );
    }

    #[test]
    fn 경로가_같아도_폴더가_다르면_다른_파일이다() {
        let comparison = compare_index(&[entry("a.txt", 1)], &[entry("sub/a.txt", 1)]);

        assert_eq!(paths(&comparison.missing), ["sub/a.txt"]);
    }

    #[test]
    fn 한쪽_목록이_비어_있는_경우() {
        let remote = vec![entry("b.txt", 2), entry("a.txt", 1)];

        let comparison = compare_index(&[], &remote);

        assert_eq!(paths(&comparison.missing), ["a.txt", "b.txt"]);
        assert_eq!(
            compare_index(&[entry("a.txt", 1)], &[]),
            IndexComparison::default()
        );
        assert_eq!(compare_index(&[], &[]), IndexComparison::default());
    }

    #[test]
    fn 상대의_삭제_기록이_내게_없으면_무시한다() {
        assert_eq!(
            compare_index(&[], &[deleted("never-had.txt", 2)]),
            IndexComparison::default()
        );
    }

    #[test]
    fn 내용이_다르고_상대의_버전이_앞서면_updated다() {
        let local = vec![with(entry("a.txt", 1), &["a"])];
        let remote = vec![with(entry("a.txt", 9), &["a", "b"])];

        let comparison = compare_index(&local, &remote);

        assert_eq!(comparison.updated, remote);
        assert!(comparison.missing.is_empty() && comparison.conflicts.is_empty());
    }

    #[test]
    fn 내용이_다르고_버전이_서로_앞서지_않으면_conflicts다() {
        let local = vec![with(entry("a.txt", 1), &["a", "a"])];
        let remote = vec![with(entry("a.txt", 9), &["a", "b"])];

        let comparison = compare_index(&local, &remote);

        assert_eq!(comparison.conflicts, remote);
        assert!(comparison.updated.is_empty() && comparison.missing.is_empty());
    }

    #[test]
    fn 내용이_다르고_내_버전이_앞서면_무시한다() {
        let local = vec![with(entry("a.txt", 1), &["a", "b"])];
        let remote = vec![with(entry("a.txt", 9), &["a"])];

        assert_eq!(compare_index(&local, &remote), IndexComparison::default());
    }

    #[test]
    fn 내용이_같고_버전이_다르면_merge다() {
        let local = vec![with(entry("a.txt", 1), &["a"])];
        let remote = vec![with(entry("a.txt", 1), &["b"])];

        let comparison = compare_index(&local, &remote);

        assert_eq!(comparison.merge, remote);
        assert!(comparison.missing.is_empty() && comparison.updated.is_empty());
        assert!(comparison.conflicts.is_empty() && comparison.deleted.is_empty());
    }

    #[test]
    fn 내용과_버전이_같으면_merge할_것이_없다() {
        let both = vec![with(entry("a.txt", 1), &["a"])];

        assert_eq!(compare_index(&both, &both), IndexComparison::default());
    }

    #[test]
    fn 둘_다_삭제_기록이고_버전이_다르면_merge다() {
        let local = vec![with(deleted("a.txt", 1), &["a"])];
        let remote = vec![with(deleted("a.txt", 1), &["b"])];

        assert_eq!(compare_index(&local, &remote).merge, remote);
    }

    #[test]
    fn 상대가_지웠고_상대의_버전이_앞서면_지울_후보다() {
        let local = vec![with(entry("a.txt", 1), &["a"]), entry("keep.txt", 2)];
        let remote = vec![with(deleted("a.txt", 1), &["a", "b"])];

        let comparison = compare_index(&local, &remote);

        assert_eq!(comparison.deleted, remote);
        assert!(comparison.missing.is_empty() && comparison.updated.is_empty());
    }

    #[test]
    fn 상대가_지웠어도_동시이면_수정이_삭제를_이겨_지우지_않는다() {
        let local = vec![with(entry("a.txt", 9), &["a", "a"])];
        let remote = vec![with(deleted("a.txt", 1), &["a", "b"])];

        assert_eq!(compare_index(&local, &remote), IndexComparison::default());
    }

    #[test]
    fn 상대가_지웠어도_내_버전이_앞서면_지우지_않는다() {
        let local = vec![with(entry("a.txt", 9), &["a", "b", "b"])];
        let remote = vec![with(deleted("a.txt", 1), &["a", "b"])];

        assert_eq!(compare_index(&local, &remote), IndexComparison::default());
    }

    #[test]
    fn 둘_다_삭제_기록이고_해시가_다르면_무시한다() {
        let local = vec![with(deleted("gone.txt", 1), &["a"])];
        let remote = vec![with(deleted("gone.txt", 2), &["a", "b"])];

        assert_eq!(compare_index(&local, &remote), IndexComparison::default());
    }

    #[test]
    fn 내가_지운_파일의_옛_내용은_다시_받지_않는다() {
        let local = vec![with(deleted("a.txt", 1), &["a", "a"])];
        let remote = vec![with(entry("a.txt", 1), &["a"])];

        assert_eq!(compare_index(&local, &remote), IndexComparison::default());
    }

    #[test]
    fn 내가_지운_뒤에_상대가_고쳤다면_상대_버전이_앞서므로_받는다() {
        let local = vec![with(deleted("a.txt", 1), &["a", "a"])];
        let remote = vec![with(entry("a.txt", 2), &["a", "a", "b"])];

        assert_eq!(compare_index(&local, &remote).missing, remote);
    }

    #[test]
    fn 같은_내용으로_되살려도_상대_버전이_앞서면_받는다() {
        let local = vec![with(deleted("a.txt", 1), &["a", "a"])];
        let remote = vec![with(entry("a.txt", 1), &["a", "a", "b"])];

        assert_eq!(compare_index(&local, &remote).missing, remote);
    }

    #[test]
    fn 내가_지우는_동안_상대가_내용을_바꿨다면_수정이_삭제를_이겨_받는다() {
        let local = vec![with(deleted("a.txt", 1), &["a", "a"])];
        let remote = vec![with(entry("a.txt", 2), &["a", "b"])];

        assert_eq!(compare_index(&local, &remote).missing, remote);
    }

    #[test]
    fn 내가_지우는_동안_상대의_내용이_그대로면_되살리지_않는다() {
        let local = vec![with(deleted("a.txt", 1), &["a", "a"])];
        let remote = vec![with(entry("a.txt", 1), &["a", "b"])];

        assert_eq!(compare_index(&local, &remote), IndexComparison::default());
    }

    #[test]
    fn 결과는_입력_순서와_상관없이_경로순이다() {
        let local = vec![
            with(entry("x.txt", 1), &["a"]),
            with(entry("y.txt", 1), &["a"]),
        ];
        let remote = vec![
            with(entry("y.txt", 2), &["a", "b"]),
            entry("c.txt", 3),
            with(entry("x.txt", 2), &["a", "b"]),
            entry("a.txt", 3),
        ];

        let comparison = compare_index(&local, &remote);

        assert_eq!(paths(&comparison.missing), ["a.txt", "c.txt"]);
        assert_eq!(paths(&comparison.updated), ["x.txt", "y.txt"]);
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
    fn 목록에서_사라진_항목은_바뀐_항목에_넣지_않는다() {
        let before = vec![entry("a.txt", 1), entry("gone.txt", 2)];
        let after = vec![entry("a.txt", 1)];

        assert!(changed_entries(&before, &after).is_empty());
    }

    #[test]
    fn 파일을_지우면_삭제_기록이_바뀐_항목이다() {
        let before = vec![entry("a.txt", 1), entry("gone.txt", 2)];
        let after = vec![entry("a.txt", 1), deleted("gone.txt", 2)];

        let changed = changed_entries(&before, &after);

        assert_eq!(changed, vec![deleted("gone.txt", 2)]);
    }

    #[test]
    fn 같은_내용으로_다시_만들어도_바뀐_항목이다() {
        let before = vec![deleted("a.txt", 1)];
        let after = vec![entry("a.txt", 1)];

        assert_eq!(changed_entries(&before, &after), vec![entry("a.txt", 1)]);
    }

    #[test]
    fn 이미_삭제_기록인_항목은_다시_알리지_않는다() {
        let before = vec![deleted("gone.txt", 2)];

        assert!(changed_entries(&before, &before).is_empty());
    }

    #[test]
    fn 응답_헤더가_목록의_크기와_해시와_같을_때만_일치한다() {
        let listed = entry("a.txt", 1); // 크기 1, 해시 [1; 32]

        assert!(matches_entry(&listed, 1, &[1; 32]));
        assert!(!matches_entry(&listed, 2, &[1; 32]), "크기가 다르다");
        assert!(!matches_entry(&listed, 1, &[2; 32]), "해시가 다르다");
    }
}
