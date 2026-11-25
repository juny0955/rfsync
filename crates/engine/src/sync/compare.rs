use std::collections::HashMap;

use rfsync_protocol::Entry;

/// 상대의 파일 목록을 내 목록과 비교한 결과. 담긴 항목은 모두 상대 쪽 것이다.
#[derive(Debug, Default, PartialEq)]
pub(super) struct IndexComparison {
    /// 받아 올 파일. 상대에게만 있거나, 내가 지운 뒤에 상대가 내용을 바꿔 새로 만든 파일이다.
    pub missing: Vec<Entry>,
    /// 양쪽에 있지만 내용(해시)이 다른 파일. 누가 최신인지 알 근거가 아직 없어 건드리지 않는다.
    pub changed: Vec<Entry>,
    /// 상대가 지웠고 내게 같은 내용으로 남아 있는 파일. 내가 지울 후보다.
    pub deleted: Vec<Entry>,
}

/// 경로를 기준으로 상대 목록을 내 목록과 비교한다. 결과는 경로순이다.
///
/// 양쪽 모두 삭제 기록(`deleted`)이 들어 있다. 삭제 기록의 해시는 지우기 직전 내용의 해시다.
/// 내용이 같으면 수정시각이 달라도 같은 파일로 본다.
///
/// | 상대 | 나 | 결과 |
/// |---|---|---|
/// | 파일 | 없음 | `missing` |
/// | 파일 | 파일, 해시 다름 | `changed` |
/// | 파일 | 삭제 기록, 해시 다름 | `missing` (내가 지운 뒤에 새로 만든 파일) |
/// | 파일 | 삭제 기록, 해시 같음 | 무시 (내가 지운 옛 내용이므로 되살리지 않는다) |
/// | 삭제 | 파일, 해시 같음 | `deleted` |
/// | 삭제 | 파일, 해시 다름 | 무시 (내가 고친 파일이므로 수정이 삭제를 이긴다) |
/// | 삭제 | 없음 또는 삭제 기록 | 무시 |
pub(super) fn compare_index(local: &[Entry], remote: &[Entry]) -> IndexComparison {
    let local_state: HashMap<&str, (&[u8; 32], bool)> = local
        .iter()
        .map(|entry| (entry.path.as_str(), (&entry.hash, entry.deleted)))
        .collect();

    let mut comparison = IndexComparison::default();
    for entry in remote {
        let mine = local_state.get(entry.path.as_str());
        match (entry.deleted, mine) {
            (false, None) => comparison.missing.push(entry.clone()),
            (false, Some((hash, false))) if **hash != entry.hash => {
                comparison.changed.push(entry.clone())
            }
            (false, Some((hash, true))) if **hash != entry.hash => {
                comparison.missing.push(entry.clone())
            }
            (true, Some((hash, false))) if **hash == entry.hash => {
                comparison.deleted.push(entry.clone())
            }
            _ => {}
        }
    }

    comparison.missing.sort_by(|a, b| a.path.cmp(&b.path));
    comparison.changed.sort_by(|a, b| a.path.cmp(&b.path));
    comparison.deleted.sort_by(|a, b| a.path.cmp(&b.path));
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
            deleted: false,
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
    fn 상대가_지웠고_내게_같은_내용으로_있으면_지울_후보다() {
        let local = vec![entry("a.txt", 1), entry("keep.txt", 2)];
        let remote = vec![deleted("a.txt", 1)];

        let comparison = compare_index(&local, &remote);

        assert_eq!(comparison.deleted, vec![deleted("a.txt", 1)]);
        assert!(comparison.missing.is_empty() && comparison.changed.is_empty());
    }

    #[test]
    fn 상대가_지웠어도_내가_고쳤으면_지우지_않는다() {
        let local = vec![entry("a.txt", 9)];
        let remote = vec![deleted("a.txt", 1)];

        assert_eq!(compare_index(&local, &remote), IndexComparison::default());
    }

    #[test]
    fn 상대의_삭제_기록이_내게_없거나_이미_삭제_기록이면_무시한다() {
        let local = vec![deleted("gone.txt", 1)];
        let remote = vec![deleted("gone.txt", 1), deleted("never-had.txt", 2)];

        assert_eq!(compare_index(&local, &remote), IndexComparison::default());
    }

    #[test]
    fn 내가_지운_파일의_옛_내용은_다시_받지_않는다() {
        let local = vec![deleted("a.txt", 1)];
        let remote = vec![entry("a.txt", 1)];

        assert_eq!(compare_index(&local, &remote), IndexComparison::default());
    }

    #[test]
    fn 내가_지운_뒤에_상대가_내용을_바꿨다면_새_파일이므로_받는다() {
        let local = vec![deleted("a.txt", 1)];
        let remote = vec![entry("a.txt", 2)];

        let comparison = compare_index(&local, &remote);

        assert_eq!(comparison.missing, vec![entry("a.txt", 2)]);
        assert!(comparison.changed.is_empty());
    }
}
