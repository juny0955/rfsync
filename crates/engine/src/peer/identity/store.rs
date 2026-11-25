use std::{fs, io, io::Write, path::Path};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::error::EngineError;

const CERT_FILE: &str = "cert.der";
const KEY_FILE: &str = "key.der";

/// DER bytes for a local identity, ordered as (certificate, private key).
pub type IdentityDer = (Vec<u8>, Vec<u8>);

pub fn load_certificate(dir: &Path) -> Result<Option<IdentityDer>, EngineError> {
    Ok(load_from(dir)?)
}

pub fn save_certificate(
    dir: &Path,
    certificate_der: &[u8],
    private_key_der: &[u8],
) -> Result<(), EngineError> {
    Ok(save_to(dir, certificate_der, private_key_der)?)
}

fn load_from(dir: &Path) -> io::Result<Option<IdentityDer>> {
    match fs::symlink_metadata(dir) {
        Ok(_) => Ok(Some((
            fs::read(dir.join(CERT_FILE))?,
            fs::read(dir.join(KEY_FILE))?,
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn save_to(dir: &Path, certificate_der: &[u8], private_key_der: &[u8]) -> io::Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "identity path already exists",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let mut builder = tempfile::Builder::new();
    builder.prefix(".iden-");
    #[cfg(unix)]
    builder.permissions(fs::Permissions::from_mode(0o700));
    let staging = builder.tempdir_in(dir.parent().unwrap_or_else(|| Path::new(".")))?;

    let mut key_options = fs::OpenOptions::new();
    key_options.write(true).create_new(true);
    #[cfg(unix)]
    key_options.mode(0o600);

    let mut key = key_options.open(staging.path().join(KEY_FILE))?;
    key.write_all(private_key_der)?;
    key.sync_all()?;
    drop(key);

    let mut cert = fs::File::create_new(staging.path().join(CERT_FILE))?;
    cert.write_all(certificate_der)?;
    cert.sync_all()?;
    drop(cert);

    fs::rename(staging.path(), dir)?;
    drop(staging.keep());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn der을_왕복하고_기존_신원을_보존한다() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("iden");
        assert!(load_from(&dir)?.is_none());

        save_to(&dir, b"certificate", b"key")?;
        assert_eq!(
            save_to(&dir, b"other certificate", b"other key")
                .expect_err("existing identity must be preserved")
                .kind(),
            io::ErrorKind::AlreadyExists
        );

        assert_eq!(
            load_from(&dir)?,
            Some((b"certificate".to_vec(), b"key".to_vec()))
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(dir.join(KEY_FILE))?.permissions().mode() & 0o777,
            0o600
        );
        Ok(())
    }

    #[test]
    fn 개인_키가_없으면_에러를_보고한다() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("iden");
        fs::create_dir(&dir)?;
        fs::write(dir.join(CERT_FILE), b"certificate")?;

        assert_eq!(
            load_from(&dir)
                .expect_err("partial identity must be reported")
                .kind(),
            io::ErrorKind::NotFound
        );
        Ok(())
    }

    #[test]
    fn 동시에_저장해도_신원은_하나만_공개된다() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("iden");
        let start = std::sync::Barrier::new(2);

        let (first, second) = std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                start.wait();
                save_to(&dir, b"certificate A", b"key A")
            });
            start.wait();
            let second = save_to(&dir, b"certificate B", b"key B");
            (first.join().expect("save thread panicked"), second)
        });

        assert_ne!(first.is_ok(), second.is_ok());
        let expected = if first.is_ok() {
            (b"certificate A".to_vec(), b"key A".to_vec())
        } else {
            (b"certificate B".to_vec(), b"key B".to_vec())
        };
        assert_eq!(load_from(&dir)?, Some(expected));
        Ok(())
    }
}
