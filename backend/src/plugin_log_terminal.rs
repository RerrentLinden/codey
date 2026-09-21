use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const HELPER_ARGUMENT: &str = "--codey-plugin-logs";
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const READ_LIMIT: u64 = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum OpenStatus {
    Opened,
    AlreadyOpen,
}

#[derive(Serialize, Deserialize)]
struct Request {
    directory: PathBuf,
    lock_path: PathBuf,
}

// 保留锁文件，确保插件卸载重装期间，各进程仍然锁定同一个文件。
fn try_lock(path: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(file)),
        Err(error)
            if error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
                || error.kind() == io::ErrorKind::WouldBlock =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub fn open(directory: &Path) -> Result<OpenStatus, String> {
    let root = codey_runtime_core::paths::default_app_state_dir().join("plugin-log-terminals");
    open_with(directory, &root, launch_terminal)
        .map_err(|error| format!("无法打开插件日志：{error:#}"))
}

fn open_with(
    directory: &Path,
    root: &Path,
    launch: impl FnOnce(&Path) -> Result<Terminal>,
) -> Result<OpenStatus> {
    fs::create_dir_all(root)?;
    let key = format!(
        "{:x}",
        Sha256::digest(directory.as_os_str().as_encoded_bytes())
    );
    let Some(_launch_lock) = try_lock(&root.join(format!("{key}.launch.lock")))? else {
        return Ok(OpenStatus::AlreadyOpen);
    };
    let lock_path = root.join(format!("{key}.running.lock"));
    let Some(probe) = try_lock(&lock_path)? else {
        return Ok(OpenStatus::AlreadyOpen);
    };
    drop(probe);

    let ticket = tempfile::Builder::new()
        .prefix("session-")
        .tempdir_in(root)?;
    let request = Request {
        directory: directory.into(),
        lock_path,
    };
    fs::write(
        ticket.path().join("request.json"),
        serde_json::to_vec(&request)?,
    )?;
    let mut terminal = launch(ticket.path())?;
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if ticket.path().join("ready").exists() {
            // 日志进程已读取请求并持有运行锁。
            if let Some(mut child) = terminal.child.take() {
                thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            return Ok(OpenStatus::Opened);
        }
        if ticket.path().join("duplicate").exists() {
            terminal.cancel();
            return Ok(OpenStatus::AlreadyOpen);
        }
        if let Some(error) = terminal.exited()? {
            anyhow::bail!("日志终端提前退出：{error}");
        }
        if Instant::now() >= deadline {
            // 取消完成前保留启动锁，避免延迟启动的日志进程继续运行。
            fs::write(ticket.path().join("cancelled"), b"")?;
            terminal.cancel();
            anyhow::bail!("日志终端启动超时");
        }
        thread::sleep(Duration::from_millis(25));
    }
}

struct Terminal {
    child: Option<Child>,
    #[cfg(target_os = "macos")]
    window_id: Option<String>,
}

impl Terminal {
    fn exited(&mut self) -> io::Result<Option<String>> {
        if let Some(child) = &mut self.child
            && let Some(status) = child.try_wait()?
        {
            // Linux 启动器可能将请求交给已有终端服务，正常退出不代表窗口关闭。
            if !status.success() || cfg!(windows) {
                return Ok(Some(status.to_string()));
            }
            self.child = None;
        }
        Ok(None)
    }

    fn cancel(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        #[cfg(target_os = "macos")]
        if let Some(id) = &self.window_id {
            let _ = Command::new("/usr/bin/osascript")
                .args(["-e", "on run argv\ntell application \"Terminal\" to close (first window whose id is (item 1 of argv as integer)) saving no\nend run", "--"])
                .arg(id)
                .output();
        }
    }
}

#[cfg(any(target_os = "macos", all(unix, test)))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(target_os = "macos")]
fn launch_terminal(ticket: &Path) -> Result<Terminal> {
    let executable = std::env::current_exe()?;
    let executable = executable.to_str().context("程序路径不是有效的 UTF-8")?;
    let ticket = ticket.to_str().context("日志会话路径不是有效的 UTF-8")?;
    // 通过参数传递命令，避免拼入 AppleScript。exec 替换 shell，
    // 关闭终端时由持锁进程直接接收 SIGHUP。
    let command = format!(
        "exec {} {} {}",
        shell_quote(executable),
        HELPER_ARGUMENT,
        shell_quote(ticket)
    );
    launch_macos_command(&command)
}

#[cfg(target_os = "macos")]
fn launch_macos_command(command: &str) -> Result<Terminal> {
    // 按 tty 字符串匹配标签页，避免 Terminal 将对象列表比较解释成类型转换。
    // 窗口编号仅供取消使用，查找失败不能让已启动的日志进程失去会话文件。
    const SCRIPT: &str = r#"on run argv
tell application "Terminal"
set logTab to do script (item 1 of argv)
set logWindowId to ""
try
set logTTY to tty of logTab
repeat with logWindow in windows
repeat with candidateTab in tabs of logWindow
if (tty of candidateTab as text) is (logTTY as text) then
set logWindowId to id of logWindow
exit repeat
end if
end repeat
if logWindowId is not "" then exit repeat
end repeat
end try
try
activate
end try
return logWindowId
end tell
end run"#;
    // AppleScript 返回后再计算握手超时，系统授权弹窗期间仍持有启动锁。
    let output = Command::new("/usr/bin/osascript")
        .args(["-e", SCRIPT, "--"])
        .arg(command)
        .output()
        .context("无法启动 macOS 终端")?;
    if !output.status.success() {
        anyhow::bail!(
            "macOS 终端启动失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let id = String::from_utf8(output.stdout)?.trim().to_owned();
    let window_id = (!id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())).then_some(id);
    Ok(Terminal {
        child: None,
        window_id,
    })
}

#[cfg(windows)]
fn launch_terminal(ticket: &Path) -> Result<Terminal> {
    // 主程序使用 GUI 子系统，日志进程取得运行锁后自行创建控制台。
    let child = Command::new(std::env::current_exe()?)
        .arg(HELPER_ARGUMENT)
        .arg(ticket)
        .spawn()
        .context("无法启动日志终端进程")?;
    Ok(Terminal { child: Some(child) })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn launch_terminal(ticket: &Path) -> Result<Terminal> {
    let executable = std::env::current_exe()?;
    for program in ["x-terminal-emulator", "xterm"] {
        match Command::new(program)
            .arg("-e")
            .arg(&executable)
            .arg(HELPER_ARGUMENT)
            .arg(ticket)
            .spawn()
        {
            Ok(child) => return Ok(Terminal { child: Some(child) }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("未找到可用终端，请安装 x-terminal-emulator 或 xterm")
}

pub fn run_if_requested() -> Result<bool> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new(HELPER_ARGUMENT)) {
        return Ok(false);
    }
    let ticket = PathBuf::from(arguments.next().context("缺少日志会话路径")?);
    anyhow::ensure!(arguments.next().is_none(), "日志会话参数无效");
    run_helper(&ticket)?;
    Ok(true)
}

fn run_helper(ticket: &Path) -> Result<()> {
    let request: Request = serde_json::from_slice(&fs::read(ticket.join("request.json"))?)?;
    let Some(_running_lock) = try_lock(&request.lock_path)? else {
        fs::write(ticket.join("duplicate"), b"")?;
        return Ok(());
    };
    if !ticket.is_dir() || ticket.join("cancelled").exists() {
        return Ok(());
    }
    #[cfg(windows)]
    unsafe {
        windows::Win32::System::Console::AllocConsole().context("无法创建日志控制台")?;
    }
    #[cfg(unix)]
    unsafe {
        // 恢复可能被桌面启动器忽略的信号，确保没有新日志时关闭窗口也会释放锁。
        libc::signal(libc::SIGHUP, libc::SIG_DFL);
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
    }
    #[cfg(unix)]
    let stdout = io::stdout();
    #[cfg(unix)]
    let mut output = stdout.lock();
    // 桌面进程可能重定向了标准输出，Windows 直接写入新建控制台。
    #[cfg(windows)]
    let mut output = {
        unsafe {
            windows::Win32::System::Console::SetConsoleOutputCP(65001)?;
        }
        OpenOptions::new().write(true).open("CONOUT$")?
    };
    writeln!(
        output,
        "Codey 插件日志：{}\n关闭此终端窗口即可停止查看。等待 host.log 和 plugin.log 更新……",
        request
            .directory
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    )?;
    output.flush()?;
    fs::write(ticket.join("ready"), b"")?;
    let mut logs = [LogTail::new("host.log"), LogTail::new("plugin.log")];
    loop {
        for log in &mut logs {
            log.poll(&request.directory.join("logs"), &mut output)?;
        }
        output.flush()?;
        thread::sleep(POLL_INTERVAL);
    }
}

struct LogTail {
    name: &'static str,
    identity: Option<(u64, u64)>,
    offset: u64,
    checkpoint: Vec<u8>,
}

impl LogTail {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            identity: None,
            offset: 0,
            checkpoint: Vec::new(),
        }
    }

    fn poll(&mut self, directory: &Path, output: &mut impl Write) -> io::Result<()> {
        let mut file = match File::open(directory.join(self.name)) {
            Ok(file) => file,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Ok(());
        }
        let identity = file_identity(&file)?;
        let replaced = self.identity != Some(identity);
        let mut truncated = metadata.len() < self.offset;
        if !replaced && !truncated && !self.checkpoint.is_empty() {
            file.seek(SeekFrom::Start(self.offset - self.checkpoint.len() as u64))?;
            let mut checkpoint = vec![0; self.checkpoint.len()];
            truncated = file.read_exact(&mut checkpoint).is_err() || checkpoint != self.checkpoint;
        }
        if replaced || truncated {
            self.offset = if self.identity.is_none() {
                metadata.len().saturating_sub(READ_LIMIT)
            } else {
                0
            };
            self.identity = Some(identity);
            self.checkpoint.clear();
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        (&mut file).take(READ_LIMIT).read_to_end(&mut bytes)?;
        if bytes.is_empty() {
            return Ok(());
        }
        writeln!(output, "\n==> {} <==", self.name)?;
        output.write_all(&bytes)?;
        self.offset += bytes.len() as u64;
        self.checkpoint = bytes[bytes.len().saturating_sub(128)..].to_vec();
        Ok(())
    }
}

#[cfg(unix)]
fn file_identity(file: &File) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn file_identity(file: &File) -> io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
        .map_err(|error| io::Error::from_raw_os_error(error.code().0))?;
    Ok((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "需要 macOS Terminal 自动化权限及 CODEY_LOG_TERMINAL_TEST_BINARY"]
    fn macos_terminal_streams_deduplicates_and_reopens() {
        struct WindowGuard(Option<Terminal>);
        impl Drop for WindowGuard {
            fn drop(&mut self) {
                if let Some(terminal) = &mut self.0 {
                    terminal.cancel();
                }
            }
        }
        let executable = std::env::var("CODEY_LOG_TERMINAL_TEST_BINARY").unwrap();
        let root = tempfile::tempdir().unwrap();
        let plugin = root.path().join("plugin");
        fs::create_dir_all(plugin.join("logs")).unwrap();
        fs::write(plugin.join("logs/host.log"), b"initial-host-marker\n").unwrap();
        let mut window = WindowGuard(None);
        let mut launch = |ticket: &Path| {
            let terminal = launch_macos_command(&format!(
                "exec {} {} {}",
                shell_quote(&executable),
                HELPER_ARGUMENT,
                shell_quote(ticket.to_str().unwrap())
            ))?;
            window.0 = Some(Terminal {
                child: None,
                window_id: terminal.window_id.clone(),
            });
            Ok(terminal)
        };
        assert_eq!(
            open_with(&plugin, root.path(), &mut launch).unwrap(),
            OpenStatus::Opened
        );
        assert_eq!(
            open_with(&plugin, root.path(), |_| panic!("重复启动终端")).unwrap(),
            OpenStatus::AlreadyOpen
        );
        fs::write(plugin.join("logs/plugin.log"), b"live-plugin-marker\n").unwrap();
        let id = window
            .0
            .as_ref()
            .unwrap()
            .window_id
            .as_ref()
            .expect("没有找到日志窗口");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let result = Command::new("/usr/bin/osascript")
                .args(["-e", "on run argv\ntell application \"Terminal\" to return contents of selected tab of (first window whose id is (item 1 of argv as integer))\nend run", "--"])
                .arg(id).output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let contents = String::from_utf8_lossy(&result.stdout);
            if contents.contains("initial-host-marker") && contents.contains("live-plugin-marker") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "终端没有显示实时日志：{contents}"
            );
            thread::sleep(POLL_INTERVAL);
        }
        window.0.as_mut().unwrap().cancel();
        window.0 = None;
        let key = format!(
            "{:x}",
            Sha256::digest(plugin.as_os_str().as_encoded_bytes())
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while try_lock(&root.path().join(format!("{key}.running.lock")))
            .unwrap()
            .is_none()
        {
            assert!(Instant::now() < deadline, "关闭窗口后没有释放运行锁");
            thread::sleep(POLL_INTERVAL);
        }
        assert_eq!(
            open_with(&plugin, root.path(), |ticket| {
                let terminal = launch_macos_command(&format!(
                    "exec {} {} {}",
                    shell_quote(&executable),
                    HELPER_ARGUMENT,
                    shell_quote(ticket.to_str().unwrap())
                ))?;
                window.0 = Some(Terminal {
                    child: None,
                    window_id: terminal.window_id.clone(),
                });
                Ok(terminal)
            })
            .unwrap(),
            OpenStatus::Opened
        );
    }

    #[test]
    fn lock_holder_child() {
        let Some(directory) = std::env::var_os("CODEY_LOG_LOCK_TEST_DIRECTORY") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let _lock = try_lock(&directory.join("lock")).unwrap().unwrap();
        fs::write(directory.join("ready"), b"").unwrap();
        loop {
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn process_exit_releases_the_session_lock() {
        let root = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "plugin_log_terminal::tests::lock_holder_child"])
            .env("CODEY_LOG_LOCK_TEST_DIRECTORY", root.path())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.path().join("ready").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let ready = root.path().join("ready").exists();
        let locked = try_lock(&root.path().join("lock")).unwrap().is_none();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(ready, "child did not acquire its lock");
        assert!(locked);
        assert!(try_lock(&root.path().join("lock")).unwrap().is_some());
    }

    #[test]
    fn active_and_starting_sessions_are_deduplicated_and_can_reopen() {
        let root = tempfile::tempdir().unwrap();
        let plugin = root.path().join("plugin");
        let launch = || {
            open_with(&plugin, root.path(), |ticket| {
                let request: Request =
                    serde_json::from_slice(&fs::read(ticket.join("request.json"))?)?;
                assert_eq!(
                    open_with(&plugin, root.path(), |_| panic!("duplicate launch"))?,
                    OpenStatus::AlreadyOpen
                );
                let lock = try_lock(&request.lock_path)?.unwrap();
                fs::write(ticket.join("ready"), b"")?;
                // 单独验证运行锁，避免测试只覆盖启动锁。
                assert!(try_lock(&request.lock_path)?.is_none());
                drop(lock);
                Ok(Terminal {
                    child: None,
                    #[cfg(target_os = "macos")]
                    window_id: None,
                })
            })
            .unwrap()
        };
        assert_eq!(launch(), OpenStatus::Opened);
        assert_eq!(launch(), OpenStatus::Opened);
        let key = format!(
            "{:x}",
            Sha256::digest(plugin.as_os_str().as_encoded_bytes())
        );
        let lock = try_lock(&root.path().join(format!("{key}.running.lock")))
            .unwrap()
            .unwrap();
        assert_eq!(
            open_with(&plugin, root.path(), |_| panic!("active launch")).unwrap(),
            OpenStatus::AlreadyOpen
        );
        drop(lock);
        assert_eq!(launch(), OpenStatus::Opened);
    }

    #[test]
    fn follows_missing_append_rotation_and_truncation_without_repeating() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("plugin.log");
        let mut tail = LogTail::new("plugin.log");
        let mut output = Vec::new();
        tail.poll(root.path(), &mut output).unwrap();
        assert!(output.is_empty());
        fs::write(&path, b"first\n").unwrap();
        tail.poll(root.path(), &mut output).unwrap();
        output.clear();
        tail.poll(root.path(), &mut output).unwrap();
        assert!(output.is_empty());
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"append\n")
            .unwrap();
        tail.poll(root.path(), &mut output).unwrap();
        assert!(String::from_utf8_lossy(&output).contains("append\n"));
        assert!(!String::from_utf8_lossy(&output).contains("first\n"));
        fs::rename(&path, root.path().join("plugin.log.1")).unwrap();
        fs::write(&path, b"rotated and longer\n").unwrap();
        output.clear();
        tail.poll(root.path(), &mut output).unwrap();
        assert!(String::from_utf8_lossy(&output).contains("rotated and longer\n"));
        fs::write(&path, b"truncated then regrown to a longer length\n").unwrap();
        output.clear();
        tail.poll(root.path(), &mut output).unwrap();
        assert!(String::from_utf8_lossy(&output).contains("truncated then regrown"));
    }

    #[cfg(unix)]
    #[test]
    fn shell_paths_round_trip_without_evaluating_metacharacters() {
        let path = "/tmp/plugin ' \" $(exit 9); `exit 8` $HOME\\换行\nend";
        let result = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf '%s' {}", shell_quote(path)))
            .output()
            .unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, path.as_bytes());
    }
}
