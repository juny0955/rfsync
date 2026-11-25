//! 두 노드가 실제 QUIC 연결로 파일 목록을 교환하고 파일을 주고받는 것을 확인하는 e2e 테스트.
//! 데몬이 쓰는 공개 API(`register_root`, `run`)만 사용한다.

use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    net::{Ipv4Addr, UdpSocket},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU16, Ordering},
    time::Duration,
};

use rfsync_engine::{DeviceId, Identity, register_root, run};
use rfsync_protocol::VersionVector;
use tempfile::TempDir;
use tokio::{
    sync::oneshot,
    time::{sleep, timeout},
};

/// 연결이 맺어지고 파일이 오가기를 기다리는 최대 시간.
const WAIT: Duration = Duration::from_secs(20);

/// `rfsync_engine`이 인증서를 만들 때 쓰는 서버 이름. 접속하는 쪽이 같은 이름으로 검증한다.
const SERVER_NAME: &str = "rfsync";

/// 비어 있는 UDP 포트를 하나 고른다. 이 파일의 테스트끼리는 같은 포트를 받지 않는다.
fn free_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(40_000);
    loop {
        let port = NEXT.fetch_add(1, Ordering::Relaxed);
        if UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).is_ok() {
            return port;
        }
    }
}

/// 동기화에 참여하는 장치 하나: 폴더, DB, 신원, 포트를 갖는다.
struct Node {
    _dir: TempDir,
    root: PathBuf,
    db_path: PathBuf,
    identity: Identity,
    port: u16,
}

impl Node {
    /// `files`(루트 기준 상대 경로와 내용)를 가진 폴더를 루트로 등록한 노드를 만든다.
    fn new(files: &[(&str, &[u8])]) -> Result<Self, Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("root");
        for (path, content) in files {
            let full = root.join(path);
            fs::create_dir_all(full.parent().ok_or("부모 폴더가 없다")?)?;
            fs::write(full, content)?;
        }
        fs::create_dir_all(&root)?;

        let db_path = dir.path().join("node.db");
        let (cert, key) = rfsync_network::generate_certificate(SERVER_NAME)?;
        register_root(&root, &db_path, DeviceId::from_cert(&cert))?;
        Ok(Self {
            _dir: dir,
            root,
            db_path,
            identity: Identity { cert, key },
            port: free_port(),
        })
    }

    /// DB에 저장된 파일의 version. 이름이 같은 파일은 없다고 가정한다. 감시가 아직 반영하지 않았으면 `None`이다.
    fn version_of(&self, name: &str) -> Option<VersionVector> {
        let conn = rfsync_storage::open(&self.db_path).unwrap();
        let root = rfsync_storage::list_roots(&conn).unwrap().remove(0);
        let file = rfsync_storage::get_files_by_root_id(&conn, root.id)
            .unwrap()
            .into_iter()
            .find(|file| file.name == name)?;
        Some(VersionVector::from_bytes(&file.version).unwrap())
    }

    /// 루트 아래의 모든 파일을 `상대 경로(/ 구분) → 내용`으로 읽는다.
    fn files(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(base: &Path, dir: &Path, found: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(base, &path, found);
                } else {
                    let relative = path.strip_prefix(base).unwrap();
                    let name = relative
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    found.insert(name, fs::read(&path).unwrap());
                }
            }
        }
        let mut found = BTreeMap::new();
        walk(&self.root, &self.root, &mut found);
        found
    }
}

/// `dialer`가 `listener`로 접속하도록 서로를 페어링된 장치로 저장한다.
/// 접속하는 쪽만 상대 주소를 저장하는 실제 페어링과 같다.
fn pair(dialer: &Node, listener: &Node) -> Result<(), Box<dyn Error>> {
    let conn = rfsync_storage::open(&dialer.db_path)?;
    let address = format!("127.0.0.1:{}", listener.port);
    rfsync_storage::upsert_device(&conn, &listener.identity.cert, Some(&address), 100)?;

    let conn = rfsync_storage::open(&listener.db_path)?;
    rfsync_storage::upsert_device(&conn, &dialer.identity.cert, None, 100)?;
    Ok(())
}

/// 두 노드의 데몬을 띄우고 `scenario`가 끝나면 둘 다 종료시켜 정상 종료를 확인한다.
/// 듣는 쪽을 먼저 poll해서 접속하는 쪽이 접속하기 전에 포트가 열려 있게 한다.
async fn with_nodes<F, Fut>(
    listener: &Node,
    dialer: &Node,
    scenario: F,
) -> Result<(), Box<dyn Error>>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
{
    let (stop_listener, listener_stopped) = oneshot::channel::<()>();
    let (stop_dialer, dialer_stopped) = oneshot::channel::<()>();

    let scenario = async {
        scenario().await;
        let _ = stop_listener.send(());
        let _ = stop_dialer.send(());
    };

    let (listener_result, dialer_result, ()) = timeout(WAIT * 2, async {
        tokio::join!(
            run(
                &listener.identity,
                listener.port,
                &listener.db_path,
                async {
                    let _ = listener_stopped.await;
                }
            ),
            run(&dialer.identity, dialer.port, &dialer.db_path, async {
                let _ = dialer_stopped.await;
            }),
            scenario
        )
    })
    .await?;

    listener_result?;
    dialer_result?;
    Ok(())
}

/// 조건이 참이 될 때까지 기다린다. 시간이 지나면 `what`을 알리며 실패한다.
async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let result = timeout(WAIT, async {
        while !condition() {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(result.is_ok(), "시간 안에 이루어지지 않았다: {what}");
}

/// 3 MiB. 읽기 버퍼(64 KiB)와 프레임 상한(1 MiB)을 모두 넘고, 바이트마다 값이 달라 순서가 틀리면 드러난다.
fn big_content() -> Vec<u8> {
    (0..3 * 1024 * 1024 + 123)
        .map(|i| (i % 251) as u8)
        .collect()
}

fn expected(files: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
    files
        .iter()
        .map(|(path, content)| ((*path).to_owned(), content.to_vec()))
        .collect()
}

#[tokio::test]
async fn each_node_receives_the_files_it_is_missing_from_the_other() -> Result<(), Box<dyn Error>> {
    let big = big_content();
    let a_files: [(&str, &[u8]); 3] = [
        ("a.txt", b"hello"),
        ("sub/deep/b.txt", b"world"),
        ("big.bin", &big),
    ];
    let b_files: [(&str, &[u8]); 1] = [("b-only.txt", b"only on b")];
    let a = Node::new(&a_files)?;
    let b = Node::new(&b_files)?;
    pair(&b, &a)?; // b가 a로 접속한다

    let all: Vec<(&str, &[u8])> = a_files.iter().chain(b_files.iter()).copied().collect();
    let want = expected(&all);

    with_nodes(&a, &b, || async {
        wait_until("b가 a의 파일을 모두 받는다", || b.files() == want).await;
        wait_until("a가 b의 파일을 받는다", || a.files() == want).await;
        // 받은 파일을 감시가 DB에 반영하는 것은 파일이 생긴 뒤 조금 걸린다.
        wait_until(
            "받은 파일의 version이 보낸 쪽과 같아진다",
            || {
                ["a.txt", "big.bin", "sub/deep/b.txt"]
                    .iter()
                    .map(|path| path.rsplit('/').next().unwrap())
                    .all(|name| {
                        b.version_of(name).is_some() && b.version_of(name) == a.version_of(name)
                    })
                    && a.version_of("b-only.txt").is_some()
                    && a.version_of("b-only.txt") == b.version_of("b-only.txt")
            },
        )
        .await;
    })
    .await?;

    // 종료 뒤에도 같은 상태이고, 받다 만 임시 파일이나 예상 밖의 파일이 없다.
    assert!(a.files() == want, "a의 최종 파일 목록이 다르다");
    assert!(b.files() == want, "b의 최종 파일 목록이 다르다");

    // 받은 파일은 보낸 쪽의 버전을 그대로 갖는다. 받은 쪽의 카운터가 올라가지 않는다.
    let mut from_a = VersionVector::default();
    from_a.bump(DeviceId::from_cert(&a.identity.cert));
    assert_eq!(b.version_of("a.txt"), Some(from_a));
    Ok(())
}

/// 6단계의 출발점. 연결된 뒤에 생긴 파일이 상대에게 전달되는지 확인한다.
/// 지금은 시작 시점의 목록만 보내므로 실패하는 것이 정상이다.
#[tokio::test]
async fn a_file_created_after_connecting_reaches_the_other_node() -> Result<(), Box<dyn Error>> {
    let a = Node::new(&[("first.txt", b"first")])?;
    let b = Node::new(&[])?;
    pair(&b, &a)?;

    with_nodes(&a, &b, || async {
        // 연결 직후의 첫 동기화가 끝난 것을 확인한다.
        wait_until("b가 first.txt를 받는다", || {
            b.files().contains_key("first.txt")
        })
        .await;

        // 연결된 뒤에 a의 폴더에 새 파일을 만든다.
        fs::write(a.root.join("later.txt"), b"later").unwrap();

        wait_until("b가 연결 뒤에 생긴 later.txt를 받는다", || {
            b.files().contains_key("later.txt")
        })
        .await;
    })
    .await?;

    assert_eq!(
        b.files(),
        expected(&[("first.txt", b"first"), ("later.txt", b"later")])
    );
    Ok(())
}

#[tokio::test]
async fn files_with_different_content_on_both_sides_are_left_untouched()
-> Result<(), Box<dyn Error>> {
    let a = Node::new(&[("x.txt", b"A-version"), ("only-a.txt", b"hello")])?;
    let b = Node::new(&[("x.txt", b"B-version")])?;
    pair(&b, &a)?;

    with_nodes(&a, &b, || async {
        // 같은 경로에 내용이 다른 파일은 어느 쪽이 최신인지 알 수 없어 건드리지 않는다.
        // 파일 하나가 전달된 것으로 동기화가 실제로 일어났음을 확인한 뒤 잠시 더 지켜본다.
        wait_until("b가 only-a.txt를 받는다", || {
            b.files().contains_key("only-a.txt")
        })
        .await;
        sleep(Duration::from_millis(300)).await;
    })
    .await?;

    assert_eq!(
        a.files(),
        expected(&[("x.txt", b"A-version"), ("only-a.txt", b"hello")])
    );
    assert_eq!(
        b.files(),
        expected(&[("x.txt", b"B-version"), ("only-a.txt", b"hello")])
    );
    Ok(())
}

#[tokio::test]
async fn nothing_is_transferred_when_both_nodes_already_have_the_same_files()
-> Result<(), Box<dyn Error>> {
    let files: [(&str, &[u8]); 2] = [("a.txt", b"hello"), ("sub/b.txt", b"world")];
    let a = Node::new(&files)?;
    let b = Node::new(&files)?;
    pair(&b, &a)?;

    with_nodes(&a, &b, || async {
        // 연결이 맺어지고 목록 비교가 끝날 시간을 준다.
        sleep(Duration::from_millis(800)).await;
    })
    .await?;

    assert_eq!(a.files(), expected(&files));
    assert_eq!(b.files(), expected(&files));
    Ok(())
}

/// 연결된 동안 한쪽이 파일을 지우면 상대에서도 지워진다.
#[tokio::test]
async fn a_file_deleted_while_connected_is_deleted_on_the_other_node() -> Result<(), Box<dyn Error>>
{
    let shared: [(&str, &[u8]); 2] = [("keep.txt", b"keep"), ("doomed.txt", b"doomed")];
    let a = Node::new(&shared)?;
    let b = Node::new(&shared)?;
    pair(&b, &a)?;

    with_nodes(&a, &b, || async {
        // 연결과 감시가 준비된 것을 새 파일이 건너가는 것으로 확인한다. 감시가 아직 준비되지 않았을 때
        // 만든 파일은 전달되지 않을 수 있으므로, 건너갈 때까지 이름을 바꿔 가며 다시 만든다.
        let mut attempt = 0;
        let ready = timeout(WAIT, async {
            loop {
                attempt += 1;
                let name = format!("ready-{attempt}.txt");
                fs::write(a.root.join(&name), b"ready").unwrap();
                sleep(Duration::from_millis(200)).await;
                if b.files().contains_key(&name) {
                    return;
                }
            }
        })
        .await;
        assert!(ready.is_ok(), "연결과 감시가 준비되지 않았다");

        // 같은 내용을 각자 따로 등록해 버전이 서로 앞서지 않았다. 연결되면 양쪽이 버전을 합친다.
        // 합치기 전에 지우면 동시에 고친 것으로 보여 삭제가 전달되지 않는다.
        wait_until("양쪽의 doomed.txt 버전이 합쳐진다", || {
            a.version_of("doomed.txt").is_some()
                && a.version_of("doomed.txt") == b.version_of("doomed.txt")
        })
        .await;

        fs::remove_file(a.root.join("doomed.txt")).unwrap();

        wait_until("b에서도 doomed.txt가 지워진다", || {
            !b.files().contains_key("doomed.txt")
        })
        .await;
        // 삭제 기록은 감시가 반영한 뒤에야 DB에 남는다. 지운 쪽의 버전과 같아질 때까지 기다린다.
        wait_until("b의 삭제 기록이 a의 것과 같아진다", || {
            b.version_of("doomed.txt").is_some()
                && b.version_of("doomed.txt") == a.version_of("doomed.txt")
        })
        .await;
    })
    .await?;

    // 상대가 지워서 지운 파일의 삭제 기록은 지운 쪽의 버전을 그대로 갖는다.
    for node in [&a, &b] {
        let files = node.files();
        assert!(!files.contains_key("doomed.txt"), "지운 파일이 남아 있다");
        assert_eq!(
            files.get("keep.txt").map(Vec::as_slice),
            Some(b"keep".as_slice())
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_file_edited_while_connected_overwrites_the_other_node() -> Result<(), Box<dyn Error>> {
    let shared: [(&str, &[u8]); 1] = [("note.txt", b"original")];
    let a = Node::new(&shared)?;
    let b = Node::new(&shared)?;
    pair(&b, &a)?;

    with_nodes(&a, &b, || async {
        // 같은 내용을 각자 따로 등록해 버전이 서로 앞서지 않는다. 합쳐진 뒤에 고쳐야 한쪽이 앞선 것이 된다.
        wait_until("양쪽의 note.txt 버전이 합쳐진다", || {
            a.version_of("note.txt").is_some()
                && a.version_of("note.txt") == b.version_of("note.txt")
        })
        .await;

        // 감시가 아직 준비되지 않았을 때 고친 것은 전달되지 않을 수 있어, 건너갈 때까지 다시 쓴다.
        let arrived = timeout(WAIT, async {
            loop {
                fs::write(a.root.join("note.txt"), b"edited on a").unwrap();
                sleep(Duration::from_millis(200)).await;
                if b.files().get("note.txt").map(Vec::as_slice) == Some(b"edited on a".as_slice()) {
                    return;
                }
            }
        })
        .await;
        assert!(arrived.is_ok(), "b의 파일이 덮어써지지 않았다");
    })
    .await?;

    let want = expected(&[("note.txt", b"edited on a")]);
    assert!(a.files() == want, "a의 최종 파일 목록이 다르다");
    assert!(b.files() == want, "b의 최종 파일 목록이 다르다");
    Ok(())
}

/// 데몬이 꺼져 있는 동안 한쪽이 파일을 지우고 상대가 고쳤다면, 켜진 뒤 고친 파일이 지운 쪽으로 돌아와야 한다.
///
/// 지금은 실패하는 것이 정상이다. 데몬이 켜질 때 핸드셰이크로 보내는 목록은 감시가 폴더를 DB에 맞추기 전의
/// 오래된 목록이라, 꺼져 있는 동안의 변경은 다음 변경 신호가 올 때까지 상대에게 알려지지 않는다.
#[tokio::test]
#[ignore = "시작 시점의 오래된 목록 문제: 꺼져 있는 동안의 변경이 전달되지 않는다"]
async fn a_file_edited_on_the_other_node_survives_a_deletion() -> Result<(), Box<dyn Error>> {
    let shared: [(&str, &[u8]); 1] = [("note.txt", b"original")];
    let a = Node::new(&shared)?;
    let b = Node::new(&shared)?;
    // 연결되기 전에 b가 파일을 고친다. 데몬이 켜지면 b의 감시가 이 변경을 DB에 반영한다.
    fs::write(b.root.join("note.txt"), b"edited on b")?;
    // 연결되기 전에 a가 파일을 지운다.
    fs::remove_file(a.root.join("note.txt"))?;
    pair(&b, &a)?;

    with_nodes(&a, &b, || async {
        wait_until("a에 b가 고친 파일이 돌아온다", || {
            a.files().get("note.txt").map(Vec::as_slice) == Some(b"edited on b".as_slice())
        })
        .await;
    })
    .await?;

    let want = expected(&[("note.txt", b"edited on b")]);
    assert!(a.files() == want, "a의 최종 파일 목록이 다르다");
    assert!(b.files() == want, "b의 최종 파일 목록이 다르다");
    Ok(())
}
