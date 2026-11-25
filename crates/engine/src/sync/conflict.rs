//! 동시에 고친 파일(충돌)의 승자를 정하고, 진 쪽을 보존할 파일 이름을 만든다.
//! 양쪽 장치가 같은 입력으로 같은 결론을 내야 하므로 순수 계산만 한다.

use rfsync_protocol::{DeviceId, Entry};

/// 충돌에서 이긴 쪽.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Winner {
    Mine,
    Theirs,
}

/// 동시에 고친 두 파일 중 이길 쪽을 정한다.
///
/// 수정시각이 더 최신인 쪽이 이기고, 같으면 `DeviceId`가 큰 쪽이 이긴다. 두 장치가 각자 이 함수를
/// 불러도(내 쪽과 상대 쪽이 뒤바뀐 채로) 같은 장치가 이기도록 입력에 대해 대칭이다.
pub(super) fn decide_winner(
    mine: &Entry,
    theirs: &Entry,
    my_id: DeviceId,
    peer_id: DeviceId,
) -> Winner {
    let modified = |entry: &Entry| (entry.modified_secs, entry.modified_nanos);
    if (modified(mine), my_id) > (modified(theirs), peer_id) {
        Winner::Mine
    } else {
        Winner::Theirs
    }
}

/// 진 쪽 내용을 보존할 파일 경로. `이름.sync-conflict-<YYYYMMDD-HHMMSS>-<장치ID 앞 4글자>.확장자`다.
///
/// 시각은 진 파일의 수정시각(UTC)이고, 장치는 그 파일을 가진 장치다. 확장자가 없으면 끝에 붙인다.
/// 폴더는 그대로 두고 파일 이름만 바꾼다.
///
/// 같은 이름에 다른 내용이 이미 있으면 `attempt`를 1, 2, …로 올려 `-1`, `-2`를 장치 ID 뒤에 붙인 다른 이름을 쓴다.
pub(super) fn conflict_path(
    path: &str,
    loser_modified_secs: i64,
    loser: DeviceId,
    attempt: u32,
) -> String {
    let (dir, name) = match path.rsplit_once('/') {
        Some((dir, name)) => (Some(dir), name),
        None => (None, path),
    };
    // 맨 앞의 점은 확장자 구분이 아니다(`.bashrc`).
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    let id: String = loser.as_bytes()[..2]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let retry = if attempt == 0 {
        String::new()
    } else {
        format!("-{attempt}")
    };
    let file = format!(
        "{stem}.sync-conflict-{}-{id}{retry}{extension}",
        format_utc(loser_modified_secs)
    );
    match dir {
        Some(dir) => format!("{dir}/{file}"),
        None => file,
    }
}

/// 유닉스 초를 `YYYYMMDD-HHMMSS`(UTC)로 바꾼다.
fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let time = secs.rem_euclid(86_400);
    // 1970-01-01 기준 일수를 날짜로 바꾼다(Howard Hinnant의 civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        time / 3_600,
        time % 3_600 / 60,
        time % 60
    )
}

#[cfg(test)]
mod tests {
    use rfsync_protocol::VersionVector;

    use super::*;

    fn entry(secs: i64, nanos: u32) -> Entry {
        Entry {
            path: "a.txt".to_owned(),
            size: 1,
            hash: [0; 32],
            modified_secs: secs,
            modified_nanos: nanos,
            deleted: false,
            version: VersionVector::default(),
        }
    }

    fn ids() -> (DeviceId, DeviceId) {
        let (a, b) = (DeviceId::from_cert(b"a"), DeviceId::from_cert(b"b"));
        if a > b { (a, b) } else { (b, a) }
    }

    #[test]
    fn 수정시각이_더_최신인_쪽이_이긴다() {
        let (big, small) = ids();
        let (older, newer) = (entry(100, 0), entry(101, 0));

        // 장치 ID가 작아도 시각이 최신이면 이긴다.
        assert_eq!(decide_winner(&older, &newer, big, small), Winner::Theirs);
        assert_eq!(decide_winner(&newer, &older, small, big), Winner::Mine);
    }

    #[test]
    fn 초가_같으면_나노초로_가린다() {
        let (big, small) = ids();

        assert_eq!(
            decide_winner(&entry(100, 5), &entry(100, 6), big, small),
            Winner::Theirs
        );
    }

    #[test]
    fn 시각이_같으면_장치_id가_큰_쪽이_이긴다() {
        let (big, small) = ids();
        let same = entry(100, 0);

        assert_eq!(decide_winner(&same, &same, big, small), Winner::Mine);
        assert_eq!(decide_winner(&same, &same, small, big), Winner::Theirs);
    }

    #[test]
    fn 두_장치가_각자_판정해도_같은_장치가_이긴다() {
        let (big, small) = ids();
        let (x, y) = (entry(100, 0), entry(100, 0));
        // x는 big의 파일, y는 small의 파일.
        let on_big = decide_winner(&x, &y, big, small);
        let on_small = decide_winner(&y, &x, small, big);

        assert_eq!(on_big, Winner::Mine);
        assert_eq!(on_small, Winner::Theirs);
    }

    #[test]
    fn 충돌_파일_이름은_확장자_앞에_붙는다() {
        let id = DeviceId::from_cert(b"a");
        let short: String = id.as_bytes()[..2]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        assert_eq!(
            conflict_path("note.txt", 0, id, 0),
            format!("note.sync-conflict-19700101-000000-{short}.txt")
        );
    }

    #[test]
    fn 폴더는_그대로_두고_파일_이름만_바꾼다() {
        let id = DeviceId::from_cert(b"a");
        let short: String = id.as_bytes()[..2]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        assert_eq!(
            conflict_path("docs/sub/note.txt", 0, id, 0),
            format!("docs/sub/note.sync-conflict-19700101-000000-{short}.txt")
        );
    }

    #[test]
    fn 확장자가_없거나_점으로_시작하면_끝에_붙인다() {
        let id = DeviceId::from_cert(b"a");
        let short: String = id.as_bytes()[..2]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        assert_eq!(
            conflict_path("README", 0, id, 0),
            format!("README.sync-conflict-19700101-000000-{short}")
        );
        assert_eq!(
            conflict_path("d/.bashrc", 0, id, 0),
            format!("d/.bashrc.sync-conflict-19700101-000000-{short}")
        );
    }

    #[test]
    fn 점이_여러_개면_마지막_점을_확장자로_본다() {
        let id = DeviceId::from_cert(b"a");
        let short: String = id.as_bytes()[..2]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        assert_eq!(
            conflict_path("a.tar.gz", 0, id, 0),
            format!("a.tar.sync-conflict-19700101-000000-{short}.gz")
        );
    }

    #[test]
    fn 시도_횟수가_늘면_장치_id_뒤에_번호가_붙는다() {
        let id = DeviceId::from_cert(b"a");
        let short: String = id.as_bytes()[..2]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        assert_eq!(
            conflict_path("d/note.txt", 0, id, 2),
            format!("d/note.sync-conflict-19700101-000000-{short}-2.txt")
        );
    }

    #[test]
    fn 시각은_utc_날짜와_시각으로_바뀐다() {
        // 2026-11-25 10:30:45 UTC, 윤일(2024-02-29), 1970년 이전.
        assert_eq!(format_utc(1_795_602_645), "20261125-103045");
        assert_eq!(format_utc(1_709_164_800), "20240229-000000");
        assert_eq!(format_utc(-1), "19691231-235959");
    }
}
