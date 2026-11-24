use std::path::{Component, Path, PathBuf};

use crate::error::EngineError;

/// 상대가 요청한 경로를 내 루트 아래의 실제 경로로 바꾼다.
///
/// 글자만 보고 판단하며 파일 시스템은 열어 보지 않는다. 일반 이름(`a`, `sub/b.txt`)으로만 이뤄진
/// 상대 경로만 받는다. 빈 경로, 절대 경로, `..`와 맨 앞의 `.`은 거절한다.
/// (`Path::components`가 가운데의 `.`과 겹친 `/`는 없애 주므로 `a/./b`는 `a/b`로 통과한다.)
///
/// 심볼릭 링크로 루트 밖을 가리키는 경우는 이 함수가 막지 못한다. 파일을 열기 직전에 따로 확인해야 한다.
pub(crate) fn resolve_in_root(root: &Path, requested: &str) -> Result<PathBuf, EngineError> {
    let relative = Path::new(requested);
    let is_plain = !requested.is_empty()
        && !requested.contains(['\0', '\\'])
        && relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)));

    if !is_plain {
        return Err(EngineError::InvalidEntryPath(relative.to_path_buf()));
    }
    Ok(root.join(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/data/root")
    }

    #[test]
    fn 일반_상대_경로는_루트_아래_경로가_된다() {
        assert_eq!(
            resolve_in_root(&root(), "a.txt").unwrap(),
            PathBuf::from("/data/root/a.txt")
        );
        assert_eq!(
            resolve_in_root(&root(), "sub/deep/한글.txt").unwrap(),
            PathBuf::from("/data/root/sub/deep/한글.txt")
        );
    }

    #[test]
    fn 위_폴더로_올라가는_경로는_거절한다() {
        for requested in ["..", "../a.txt", "sub/../../a.txt", "sub/..", "a/../b"] {
            assert!(
                resolve_in_root(&root(), requested).is_err(),
                "{requested:?}가 통과했다"
            );
        }
    }

    #[test]
    fn 절대_경로는_거절한다() {
        for requested in ["/etc/passwd", "/", "//a.txt"] {
            assert!(
                resolve_in_root(&root(), requested).is_err(),
                "{requested:?}가 통과했다"
            );
        }
    }

    #[test]
    fn 빈_경로와_맨_앞의_현재_폴더_조각은_거절한다() {
        for requested in ["", ".", "./a.txt"] {
            assert!(
                resolve_in_root(&root(), requested).is_err(),
                "{requested:?}가 통과했다"
            );
        }
    }

    #[test]
    fn 경로_가운데의_점이나_겹친_슬래시는_루트_안을_가리키므로_통과한다() {
        // `Path::components`가 가운데의 `.`과 겹친 `/`를 없애 버려 `a/b`와 같은 경로가 된다.
        assert_eq!(
            resolve_in_root(&root(), "a/./b").unwrap(),
            PathBuf::from("/data/root/a/b")
        );
        assert_eq!(
            resolve_in_root(&root(), "a//b").unwrap(),
            PathBuf::from("/data/root/a/b")
        );
    }

    #[test]
    fn 널_문자나_역슬래시가_든_경로는_거절한다() {
        // 와이어 경로의 구분자는 `/`뿐이다. 유닉스에서는 `\`가 일반 글자라 따로 막아야 한다.
        for requested in ["a\0b", "sub\\a.txt", "..\\a.txt"] {
            assert!(
                resolve_in_root(&root(), requested).is_err(),
                "{requested:?}가 통과했다"
            );
        }
    }
}
