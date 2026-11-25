//! 아무것도 등록되지 않은 데몬 둘을 띄우고, CLI가 하듯 소켓으로 루트 등록과 페어링을 시킨 뒤
//! 파일이 건너가는지 확인하는 e2e 테스트. 데몬을 다시 시작하지 않고 모두 실행 중에 이루어진다.
#![cfg(unix)]

use std::{
    error::Error,
    fs,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU16, Ordering},
    time::Duration,
};

use rfsync_engine::{Identity, run, socket_path};
use rfsync_ipc::{Request, Response, request, request_streaming};
use tempfile::TempDir;
use tokio::{
    sync::{mpsc, oneshot},
    task::spawn_blocking,
    time::{sleep, timeout},
};

const WAIT: Duration = Duration::from_secs(20);

static NEXT_PORT: AtomicU16 = AtomicU16::new(45_000);

struct Daemon {
    _dir: TempDir,
    root: PathBuf,
    db_path: PathBuf,
    identity: Identity,
    port: u16,
}

impl Daemon {
    fn new(files: &[(&str, &[u8])]) -> Result<Self, Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("root");
        fs::create_dir_all(&root)?;
        for (name, content) in files {
            fs::write(root.join(name), content)?;
        }
        let (cert, key) = rfsync_network::generate_certificate("rfsync")?;
        // 같은 프로세스의 데몬끼리 같은 포트를 받지 않도록 번호를 하나씩 올려가며 UDP·TCP 모두 비어 있는 포트를 고른다.
        let port = loop {
            let port = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
            if UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).is_ok()
                && std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).is_ok()
            {
                break port;
            }
        };
        Ok(Self {
            db_path: dir.path().join("daemon.db"),
            _dir: dir,
            root,
            identity: Identity { cert, key },
            port,
        })
    }

    fn socket(&self) -> PathBuf {
        socket_path(&self.db_path)
    }
}

/// 소켓으로 요청 하나를 보내고 마지막 응답을 받는다. CLI와 같은 동기 클라이언트를 쓴다.
async fn ask(socket: PathBuf, req: Request) -> Response {
    spawn_blocking(move || request(&socket, &req).unwrap())
        .await
        .unwrap()
}

async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let result = timeout(WAIT, async {
        while !condition() {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(result.is_ok(), "시간 안에 이루어지지 않았다: {what}");
}

fn read(root: &Path, name: &str) -> Option<Vec<u8>> {
    fs::read(root.join(name)).ok()
}

#[tokio::test]
async fn 빈_상태로_시작한_데몬들은_소켓으로_설정되고_페어링된다()
-> Result<(), Box<dyn Error>> {
    let a = Daemon::new(&[("a.txt", b"from a")])?;
    let b = Daemon::new(&[])?;
    let (stop_a, a_stopped) = oneshot::channel::<()>();
    let (stop_b, b_stopped) = oneshot::channel::<()>();

    let scenario = async {
        wait_until("데몬 소켓", || {
            a.socket().exists() && b.socket().exists()
        })
        .await;

        // 루트 등록: 데몬이 이미 떠 있는 상태에서 이루어진다.
        assert_eq!(
            ask(
                a.socket(),
                Request::AddRoot {
                    path: a.root.clone()
                }
            )
            .await,
            Response::Ok
        );
        assert_eq!(
            ask(
                b.socket(),
                Request::AddRoot {
                    path: b.root.clone()
                }
            )
            .await,
            Response::Ok
        );

        // 페어링: a가 코드를 내고 b가 그 코드로 접속한다.
        let (code_tx, mut code_rx) = mpsc::unbounded_channel();
        let socket = a.socket();
        let listening = spawn_blocking(move || {
            request_streaming(&socket, &Request::PairListen, |response| {
                if let Response::PairingCode(code) = response {
                    let _ = code_tx.send(code.clone());
                }
            })
            .unwrap()
        });
        let code = timeout(WAIT, code_rx.recv()).await.unwrap().unwrap();
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, a.port));
        assert_eq!(
            ask(b.socket(), Request::PairConnect { addr, code }).await,
            Response::Ok
        );
        assert_eq!(listening.await.unwrap(), Response::Ok);

        // 데몬을 다시 시작하지 않아도 b가 a에 연결해 파일을 받는다.
        wait_until("b가 a.txt를 받음", || {
            read(&b.root, "a.txt").as_deref() == Some(b"from a".as_slice())
        })
        .await;

        // 연결된 뒤 만든 파일도 건너간다.
        fs::write(b.root.join("b.txt"), b"from b")?;
        wait_until("a가 b.txt를 받음", || {
            read(&a.root, "b.txt").as_deref() == Some(b"from b".as_slice())
        })
        .await;

        let _ = stop_a.send(());
        let _ = stop_b.send(());
        Ok::<(), Box<dyn Error>>(())
    };

    let (a_result, b_result, scenario) = timeout(WAIT * 3, async {
        tokio::join!(
            run(&a.identity, a.port, &a.db_path, async {
                let _ = a_stopped.await;
            }),
            run(&b.identity, b.port, &b.db_path, async {
                let _ = b_stopped.await;
            }),
            scenario
        )
    })
    .await?;

    a_result?;
    b_result?;
    scenario?;
    // 정상 종료하면 소켓 파일이 남지 않는다.
    assert!(!a.socket().exists() && !b.socket().exists());
    Ok(())
}
