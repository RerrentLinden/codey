use std::fs;
#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// 与目标同目录的一次性临时文件名。随机 UUID 保证同一进程、同一毫秒内的
/// 并发写者也不会覆盖对方的临时文件。
pub(crate) fn unique_temp_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "codey".to_string());
    parent.join(format!(".{file_name}.codey-{}.tmp", uuid::Uuid::new_v4()))
}

/// 把写好的临时文件原子替换到目标位置。`std::fs::rename` 在部分 Windows
/// 版本无法替换被打开的目标文件，此时保持同目录先删后移；失败路径总是清理
/// 临时文件。此前七个模块各自实现这段回退且互有漂移。
pub(crate) fn persist_temp_file(temp: &Path, destination: &Path) -> std::io::Result<()> {
    match fs::rename(temp, destination) {
        Ok(()) => Ok(()),
        Err(error) => {
            #[cfg(windows)]
            if destination.exists() {
                let result =
                    fs::remove_file(destination).and_then(|_| fs::rename(temp, destination));
                if result.is_err() {
                    let _ = fs::remove_file(temp);
                }
                return result;
            }
            let _ = fs::remove_file(temp);
            Err(error)
        }
    }
}

fn atomic_write_with(
    destination: &Path,
    write_temp: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let temp = unique_temp_path(destination);
    if let Err(error) = write_temp(&temp) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    persist_temp_file(&temp, destination)
}

pub(crate) fn atomic_write(destination: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    atomic_write_with(destination, |temp| fs::write(temp, bytes))?;
    Ok(())
}

pub(crate) fn atomic_write_private(destination: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    atomic_write_with(destination, |temp| {
        #[cfg(unix)]
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(temp)?;
            file.write_all(bytes)?;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(not(unix))]
        fs::write(temp, bytes)?;
        Ok(())
    })?;
    Ok(())
}

pub(crate) fn atomic_write_private_with_parent(
    destination: &Path,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", destination.display()))?;
    fs::create_dir_all(parent)?;
    atomic_write_private(destination, bytes)
}

pub(crate) fn atomic_write_preserving_permissions(
    destination: &Path,
    bytes: &[u8],
) -> anyhow::Result<()> {
    atomic_write_with(destination, |temp| {
        fs::write(temp, bytes)?;
        if let Ok(metadata) = fs::metadata(destination) {
            fs::set_permissions(temp, metadata.permissions())?;
        }
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_paths_are_unique_for_the_same_destination() {
        let destination = Path::new("state/config.toml");
        let first = unique_temp_path(destination);
        let second = unique_temp_path(destination);

        assert_ne!(first, second);
        assert_eq!(first.parent(), destination.parent());
        assert_eq!(second.parent(), destination.parent());
    }

    #[test]
    fn persisted_temp_file_replaces_destination_and_removes_temp() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("state.json");
        let temp = unique_temp_path(&destination);
        fs::write(&destination, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        persist_temp_file(&temp, &destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert!(!temp.exists());
    }

    #[test]
    fn failed_persist_removes_the_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let temp = directory.path().join("state.tmp");
        let destination = directory.path().join("missing").join("state.json");
        fs::write(&temp, b"temporary").unwrap();

        let error = persist_temp_file(&temp, &destination).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(!temp.exists());
        assert!(!destination.exists());
    }

    #[test]
    fn atomic_write_replaces_destination() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("state.json");
        fs::write(&destination, b"old").unwrap();

        atomic_write(&destination, b"new").unwrap();

        assert_eq!(fs::read(destination).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn private_atomic_write_uses_private_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("secret.json");

        atomic_write_private(&destination, b"secret").unwrap();

        let mode = fs::metadata(destination).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_can_preserve_existing_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("state.json");
        fs::write(&destination, b"old").unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o640)).unwrap();

        atomic_write_preserving_permissions(&destination, b"new").unwrap();

        let mode = fs::metadata(destination).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }
}
