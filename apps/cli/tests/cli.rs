use std::{error::Error, fs, process::Command, time::SystemTime};

#[test]
fn add_fails_with_a_hint_when_the_daemon_is_not_running() -> Result<(), Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!(
        "rfsync-cli-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_nanos()
    ));
    fs::create_dir(&dir)?;
    fs::create_dir(dir.join("root"))?;

    let output = Command::new(env!("CARGO_BIN_EXE_rfsync"))
        .current_dir(&dir)
        .args(["add", "root"])
        .output()?;

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("데몬에 연결하지 못했습니다"));
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn add_fails_when_the_path_does_not_exist() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_rfsync"))
        .current_dir(std::env::temp_dir())
        .args(["add", "rfsync-no-such-folder"])
        .output()?;

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("rfsync-no-such-folder"));
    Ok(())
}

#[test]
fn help_succeeds_and_missing_argument_fails() -> Result<(), Box<dyn Error>> {
    let binary = env!("CARGO_BIN_EXE_rfsync");
    assert!(
        Command::new(binary)
            .arg("--help")
            .output()?
            .status
            .success()
    );
    assert!(!Command::new(binary).output()?.status.success());
    Ok(())
}
