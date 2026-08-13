use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::commands::{self, AppState, UpdateCandidate, UpdateDownload};
use crate::native_update_ui::NativeUpdateUi;

const CHECK_PROMPT_DELAY: Duration = Duration::from_millis(300);
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupUpdateOutcome {
    Continue,
    InstallScheduled,
}

#[async_trait]
trait StartupUpdateUi: Send + Sync {
    fn show_status(&self, message: &str) -> Result<(), String>;
    fn hide_status(&self) -> Result<(), String>;
    async fn confirm_update(
        &self,
        current_version: &str,
        latest_version: &str,
    ) -> Result<bool, String>;
    async fn show_update_failure(&self, error: &str) -> Result<(), String>;
}

#[async_trait]
impl StartupUpdateUi for NativeUpdateUi {
    fn show_status(&self, message: &str) -> Result<(), String> {
        NativeUpdateUi::show_status(self, message)
    }

    fn hide_status(&self) -> Result<(), String> {
        NativeUpdateUi::hide_status(self)
    }

    async fn confirm_update(
        &self,
        current_version: &str,
        latest_version: &str,
    ) -> Result<bool, String> {
        NativeUpdateUi::confirm_update(self, current_version, latest_version).await
    }

    async fn show_update_failure(&self, error: &str) -> Result<(), String> {
        NativeUpdateUi::show_update_failure(self, error).await
    }
}

#[async_trait]
trait StartupUpdateBackend: Send + Sync {
    async fn check(&self) -> Result<UpdateCandidate, String>;
    async fn download(&self, candidate: &UpdateCandidate) -> Result<UpdateDownload, String>;
    async fn install(&self, file_path: &str) -> Result<(), String>;
}

struct LiveBackend<'a> {
    state: &'a Arc<AppState>,
}

#[async_trait]
impl StartupUpdateBackend for LiveBackend<'_> {
    async fn check(&self) -> Result<UpdateCandidate, String> {
        commands::check_for_update_candidate(self.state).await
    }

    async fn download(&self, candidate: &UpdateCandidate) -> Result<UpdateDownload, String> {
        commands::download_update_candidate(self.state, candidate).await
    }

    async fn install(&self, file_path: &str) -> Result<(), String> {
        commands::start_downloaded_update(self.state, file_path).await
    }
}

pub async fn run(state: &Arc<AppState>, ui: &NativeUpdateUi) -> StartupUpdateOutcome {
    run_with(&LiveBackend { state }, ui).await
}

async fn run_with(
    backend: &impl StartupUpdateBackend,
    ui: &impl StartupUpdateUi,
) -> StartupUpdateOutcome {
    let check = tokio::time::timeout(CHECK_TIMEOUT, backend.check());
    tokio::pin!(check);
    let mut checking_visible = false;
    let result = tokio::select! {
        result = &mut check => result,
        _ = tokio::time::sleep(CHECK_PROMPT_DELAY) => {
            checking_visible = ui
                .show_status("正在检查更新，请稍候…")
                .is_ok();
            check.await
        }
    };
    if checking_visible {
        let _ = ui.hide_status();
    }
    let candidate = match result {
        Ok(Ok(candidate)) => candidate,
        Ok(Err(_)) | Err(_) => return StartupUpdateOutcome::Continue,
    };
    if !candidate.check.update_available || candidate.check.selected_asset.is_none() {
        return StartupUpdateOutcome::Continue;
    }

    let should_update = ui
        .confirm_update(
            &candidate.check.current_version,
            &candidate.check.latest_version,
        )
        .await
        .unwrap_or_default();
    if !should_update {
        return StartupUpdateOutcome::Continue;
    }

    let _ = ui.show_status(&format!(
        "正在下载并验证 Codey v{}，请稍候…",
        candidate.check.latest_version
    ));
    let download = tokio::time::timeout(DOWNLOAD_TIMEOUT, backend.download(&candidate)).await;
    let download = match download {
        Ok(Ok(download)) => download,
        Ok(Err(error)) => {
            let _ = ui.hide_status();
            let _ = ui.show_update_failure(&error).await;
            return StartupUpdateOutcome::Continue;
        }
        Err(_) => {
            let _ = ui.hide_status();
            let _ = ui
                .show_update_failure("下载更新超时，请进入 Codex 后重试")
                .await;
            return StartupUpdateOutcome::Continue;
        }
    };
    if let Err(error) = backend.install(&download.file_path).await {
        let _ = ui.hide_status();
        let _ = ui.show_update_failure(&error).await;
        return StartupUpdateOutcome::Continue;
    }
    let _ = ui.hide_status();
    StartupUpdateOutcome::InstallScheduled
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use super::*;
    use crate::commands::{UpdateAssetInfo, UpdateCheck};

    #[derive(Default)]
    struct FakeUi {
        messages: Mutex<Vec<String>>,
        hides: AtomicUsize,
        confirmations: AtomicUsize,
        failures: Mutex<Vec<String>>,
        confirm: AtomicBool,
        status_error: AtomicBool,
    }

    #[async_trait]
    impl StartupUpdateUi for FakeUi {
        fn show_status(&self, message: &str) -> Result<(), String> {
            if self.status_error.load(Ordering::Relaxed) {
                return Err("状态窗口不可用".to_string());
            }
            self.messages.lock().unwrap().push(message.to_string());
            Ok(())
        }

        fn hide_status(&self) -> Result<(), String> {
            self.hides.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        async fn confirm_update(
            &self,
            _current_version: &str,
            _latest_version: &str,
        ) -> Result<bool, String> {
            self.confirmations.fetch_add(1, Ordering::Relaxed);
            Ok(self.confirm.load(Ordering::Relaxed))
        }

        async fn show_update_failure(&self, error: &str) -> Result<(), String> {
            self.failures.lock().unwrap().push(error.to_string());
            Ok(())
        }
    }

    struct FakeBackend {
        check_delay: Duration,
        check: Result<UpdateCandidate, String>,
        download: Result<UpdateDownload, String>,
        download_delay: Duration,
        downloads: AtomicUsize,
        installs: AtomicUsize,
        install_error: Option<String>,
    }

    #[async_trait]
    impl StartupUpdateBackend for FakeBackend {
        async fn check(&self) -> Result<UpdateCandidate, String> {
            tokio::time::sleep(self.check_delay).await;
            self.check.clone()
        }

        async fn download(&self, _candidate: &UpdateCandidate) -> Result<UpdateDownload, String> {
            self.downloads.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(self.download_delay).await;
            self.download.clone()
        }

        async fn install(&self, _file_path: &str) -> Result<(), String> {
            self.installs.fetch_add(1, Ordering::Relaxed);
            match &self.install_error {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }
    }

    fn asset() -> UpdateAssetInfo {
        UpdateAssetInfo {
            platform: "macos".to_string(),
            arch: "arm64".to_string(),
            package_type: "app-zip".to_string(),
            file_name: "Codey.zip".to_string(),
            url: "https://updates.example.test/Codey.zip".to_string(),
            sha256: "a".repeat(64),
            size: 42,
        }
    }

    fn candidate(update_available: bool, installable: bool) -> UpdateCandidate {
        UpdateCandidate {
            check: UpdateCheck {
                current_version: "1.0.0".to_string(),
                latest_version: if update_available {
                    "2.0.0".to_string()
                } else {
                    "1.0.0".to_string()
                },
                update_available,
                selected_asset: installable.then(asset),
            },
        }
    }

    fn download() -> UpdateDownload {
        let asset = asset();
        UpdateDownload {
            latest_version: "2.0.0".to_string(),
            file_path: "/tmp/Codey.zip".to_string(),
            file_name: asset.file_name.clone(),
            size: asset.size,
            sha256: asset.sha256.clone(),
            asset,
        }
    }

    fn backend(check_delay: Duration, candidate: UpdateCandidate) -> FakeBackend {
        FakeBackend {
            check_delay,
            check: Ok(candidate),
            download: Ok(download()),
            download_delay: Duration::ZERO,
            downloads: AtomicUsize::new(0),
            installs: AtomicUsize::new(0),
            install_error: None,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fast_no_update_skips_native_status() {
        let backend = backend(Duration::ZERO, candidate(false, false));
        let ui = FakeUi::default();

        assert_eq!(
            run_with(&backend, &ui).await,
            StartupUpdateOutcome::Continue
        );
        assert!(ui.messages.lock().unwrap().is_empty());
        assert_eq!(ui.hides.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn slow_check_shows_and_closes_native_status() {
        let backend = backend(Duration::from_secs(1), candidate(false, false));
        let ui = FakeUi::default();

        assert_eq!(
            run_with(&backend, &ui).await,
            StartupUpdateOutcome::Continue
        );
        assert_eq!(
            ui.messages.lock().unwrap().as_slice(),
            ["正在检查更新，请稍候…"]
        );
        assert_eq!(ui.hides.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn check_timeout_is_fail_open() {
        let backend = backend(Duration::from_secs(20), candidate(true, true));
        let ui = FakeUi::default();

        assert_eq!(
            run_with(&backend, &ui).await,
            StartupUpdateOutcome::Continue
        );
        assert_eq!(ui.hides.load(Ordering::Relaxed), 1);
        assert_eq!(ui.confirmations.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn status_window_failure_does_not_cancel_the_check() {
        let backend = backend(Duration::from_secs(1), candidate(true, true));
        let ui = FakeUi::default();
        ui.status_error.store(true, Ordering::Relaxed);

        assert_eq!(
            run_with(&backend, &ui).await,
            StartupUpdateOutcome::Continue
        );
        assert_eq!(ui.confirmations.load(Ordering::Relaxed), 1);
        assert_eq!(backend.downloads.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn later_keeps_update_without_downloading() {
        let backend = backend(Duration::ZERO, candidate(true, true));
        let ui = FakeUi::default();

        assert_eq!(
            run_with(&backend, &ui).await,
            StartupUpdateOutcome::Continue
        );
        assert_eq!(ui.confirmations.load(Ordering::Relaxed), 1);
        assert_eq!(backend.downloads.load(Ordering::Relaxed), 0);
        assert_eq!(backend.installs.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn confirmed_update_downloads_and_schedules_install() {
        let backend = backend(Duration::ZERO, candidate(true, true));
        let ui = FakeUi::default();
        ui.confirm.store(true, Ordering::Relaxed);

        assert_eq!(
            run_with(&backend, &ui).await,
            StartupUpdateOutcome::InstallScheduled
        );
        assert_eq!(backend.downloads.load(Ordering::Relaxed), 1);
        assert_eq!(backend.installs.load(Ordering::Relaxed), 1);
        assert_eq!(
            ui.messages.lock().unwrap().as_slice(),
            ["正在下载并验证 Codey v2.0.0，请稍候…"]
        );
        assert_eq!(ui.hides.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn missing_download_status_window_does_not_cancel_confirmed_update() {
        let backend = backend(Duration::ZERO, candidate(true, true));
        let ui = FakeUi::default();
        ui.confirm.store(true, Ordering::Relaxed);
        ui.status_error.store(true, Ordering::Relaxed);

        assert_eq!(
            run_with(&backend, &ui).await,
            StartupUpdateOutcome::InstallScheduled
        );
        assert_eq!(backend.downloads.load(Ordering::Relaxed), 1);
        assert_eq!(backend.installs.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn download_failure_is_reported_and_continues() {
        let mut backend = backend(Duration::ZERO, candidate(true, true));
        backend.download = Err("下载损坏".to_string());
        let ui = FakeUi::default();
        ui.confirm.store(true, Ordering::Relaxed);

        assert_eq!(
            run_with(&backend, &ui).await,
            StartupUpdateOutcome::Continue
        );
        assert_eq!(ui.failures.lock().unwrap().as_slice(), ["下载损坏"]);
        assert_eq!(backend.installs.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn download_timeout_is_reported_and_continues() {
        let mut backend = backend(Duration::ZERO, candidate(true, true));
        backend.download_delay = Duration::from_secs(301);
        let ui = FakeUi::default();
        ui.confirm.store(true, Ordering::Relaxed);

        assert_eq!(
            run_with(&backend, &ui).await,
            StartupUpdateOutcome::Continue
        );
        assert_eq!(
            ui.failures.lock().unwrap().as_slice(),
            ["下载更新超时，请进入 Codex 后重试"]
        );
        assert_eq!(backend.installs.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn installer_failure_is_reported_and_continues() {
        let mut backend = backend(Duration::ZERO, candidate(true, true));
        backend.install_error = Some("无法启动安装器".to_string());
        let ui = FakeUi::default();
        ui.confirm.store(true, Ordering::Relaxed);

        assert_eq!(
            run_with(&backend, &ui).await,
            StartupUpdateOutcome::Continue
        );
        assert_eq!(ui.failures.lock().unwrap().as_slice(), ["无法启动安装器"]);
        assert_eq!(backend.installs.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn update_without_platform_asset_only_sets_passive_state() {
        let backend = backend(Duration::ZERO, candidate(true, false));
        let ui = FakeUi::default();

        assert_eq!(
            run_with(&backend, &ui).await,
            StartupUpdateOutcome::Continue
        );
        assert_eq!(ui.confirmations.load(Ordering::Relaxed), 0);
        assert_eq!(backend.downloads.load(Ordering::Relaxed), 0);
    }
}
