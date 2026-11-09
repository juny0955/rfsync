use std::{error::Error, fs, process::Command, time::SystemTime};

#[test]
fn registers_root_and_reports_duplicate() -> Result<(), Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!(
        "rfsync-cli-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_nanos()
    ));
    fs::create_dir(&dir)?;
    fs::create_dir(dir.join("root"))?;
    fs::write(dir.join("root/file"), b"test")?;

    let run = || {
        Command::new(env!("CARGO_BIN_EXE_rfsync"))
            .current_dir(&dir)
            .arg("root")
            .output()
    };
    let first = run()?;
    let duplicate = run()?;

    assert!(first.status.success());
    assert!(dir.join("example.db").exists());
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("이미 등록된 root"));
    fs::remove_dir_all(dir)?;
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
