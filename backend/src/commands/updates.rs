#[cfg(target_os = "macos")]
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::USER_AGENT;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::AppState;
use crate::config::ConfigStore;

#[derive(Clone, Debug, Deserialize)]
pub(super) struct UpdateManifest {
    schema_version: u32,
    version: String,
    tag: String,
    assets: Vec<UpdateManifestAsset>,
}

#[derive(Clone, Debug, Deserialize)]
struct UpdateManifestAsset {
    platform: String,
    arch: String,
    package_type: String,
    file_name: String,
    url: String,
    sha256: String,
    size: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateCheck {
    pub(crate) current_version: String,
    pub(crate) latest_version: String,
    pub(crate) update_available: bool,
    pub(crate) selected_asset: Option<UpdateAssetInfo>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateAssetInfo {
    pub(crate) platform: String,
    pub(crate) arch: String,
    pub(crate) package_type: String,
    pub(crate) file_name: String,
    pub(crate) url: String,
    pub(crate) sha256: String,
    pub(crate) size: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateDownload {
    pub(crate) latest_version: String,
    pub(crate) file_path: String,
    pub(crate) file_name: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
    pub(crate) asset: UpdateAssetInfo,
}

#[derive(Clone, Debug)]
pub(crate) struct UpdateCandidate {
    pub(crate) check: UpdateCheck,
}

pub async fn check_for_updates(state: &Arc<AppState>) -> Result<Value, String> {
    let candidate = check_for_update_candidate(state).await?;
    serde_json::to_value(candidate.check).map_err(|error| error.to_string())
}

pub async fn download_update(state: &Arc<AppState>) -> Result<Value, String> {
    let candidate = check_for_update_candidate(state).await?;
    let download = download_update_candidate(state, &candidate).await?;
    serde_json::to_value(download).map_err(|error| error.to_string())
}

pub(crate) async fn check_for_update_candidate(
    state: &Arc<AppState>,
) -> Result<UpdateCandidate, String> {
    let manifest = fetch_configured_update_manifest(state).await?;
    let check = assess_update_manifest(env!("CARGO_PKG_VERSION"), &manifest)?;
    *state.available_update.write().await = check.update_available.then(|| check.clone());
    Ok(UpdateCandidate { check })
}

pub(crate) async fn download_update_candidate(
    state: &Arc<AppState>,
    candidate: &UpdateCandidate,
) -> Result<UpdateDownload, String> {
    if !candidate.check.update_available {
        return Err(format!(
            "当前已是最新版本 v{}",
            candidate.check.current_version
        ));
    }
    let asset = candidate
        .check
        .selected_asset
        .as_ref()
        .ok_or_else(|| "没有适用于当前系统的可安装更新包".to_string())?;
    let file_path = download_update_asset(
        &state.http_client,
        &state.store,
        &candidate.check.latest_version,
        asset,
    )
    .await?;
    Ok(UpdateDownload {
        latest_version: candidate.check.latest_version.clone(),
        file_path: file_path.to_string_lossy().to_string(),
        file_name: asset.file_name.clone(),
        size: asset.size,
        sha256: asset.sha256.clone(),
        asset: asset.clone(),
    })
}

pub async fn install_downloaded_update(
    state: &Arc<AppState>,
    file_path: String,
) -> Result<Value, String> {
    start_downloaded_update(state, &file_path)?;
    let shutdown_state = Arc::clone(state);
    tokio::spawn(async move {
        // Let the bridge deliver the response before Codex/Codey starts
        // normal shutdown and releases the executable for replacement.
        tokio::time::sleep(Duration::from_millis(250)).await;
        shutdown_state.request_update_shutdown();
    });
    Ok(json!({"status":"installing"}))
}

pub(crate) fn start_downloaded_update(state: &AppState, file_path: &str) -> Result<(), String> {
    let update_path = validate_downloaded_update_path(&state.store, &file_path)?;
    spawn_update_installer(&update_path)
}

async fn fetch_configured_update_manifest(state: &Arc<AppState>) -> Result<UpdateManifest, String> {
    let manifest_url = state
        .config
        .read()
        .await
        .update_manifest_url
        .trim()
        .to_string();
    if manifest_url.is_empty() {
        return Err("内置更新地址未配置，请检查构建配置".to_string());
    }

    let url = reqwest::Url::parse(&manifest_url)
        .map_err(|_| "更新地址必须是有效的 HTTPS URL".to_string())?;
    if url.scheme() != "https" {
        return Err("更新地址必须使用 HTTPS".to_string());
    }

    let response = state
        .http_client
        .get(url)
        .header(
            USER_AGENT,
            format!("Codey/{} update-check", env!("CARGO_PKG_VERSION")),
        )
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| format!("检查更新失败：{error}"))?
        .error_for_status()
        .map_err(|error| format!("更新地址返回异常：{error}"))?;
    if response.url().scheme() != "https" {
        return Err("更新地址重定向到了非 HTTPS 地址".to_string());
    }

    let manifest = response
        .json::<UpdateManifest>()
        .await
        .map_err(|error| format!("更新清单格式无效：{error}"))?;
    Ok(manifest)
}

pub(super) fn assess_update_manifest(
    current_version: &str,
    manifest: &UpdateManifest,
) -> Result<UpdateCheck, String> {
    if manifest.schema_version != 1 {
        return Err(format!("不支持的更新清单版本：{}", manifest.schema_version));
    }
    if manifest.tag != format!("v{}", manifest.version) {
        return Err("更新清单的版本和标签不一致".to_string());
    }
    if manifest.assets.is_empty() {
        return Err("更新清单没有可下载的安装包".to_string());
    }
    for asset in &manifest.assets {
        validate_update_asset(asset)?;
    }

    let current =
        Version::parse(current_version).map_err(|error| format!("当前版本格式无效：{error}"))?;
    let latest = Version::parse(&manifest.version)
        .map_err(|error| format!("更新清单版本格式无效：{error}"))?;
    Ok(UpdateCheck {
        current_version: current.to_string(),
        latest_version: latest.to_string(),
        update_available: latest > current,
        selected_asset: selected_update_asset(&manifest.assets).map(|asset| asset_info(&asset)),
    })
}

fn validate_update_asset(asset: &UpdateManifestAsset) -> Result<(), String> {
    if asset.platform.trim().is_empty()
        || asset.arch.trim().is_empty()
        || asset.package_type.trim().is_empty()
        || asset.file_name.trim().is_empty()
        || asset.size == 0
    {
        return Err("更新清单包含不完整的安装包信息".to_string());
    }
    if asset.file_name.contains(['/', '\\'])
        || Path::new(&asset.file_name).components().count() != 1
    {
        return Err(format!("安装包文件名无效：{}", asset.file_name));
    }
    let url = reqwest::Url::parse(&asset.url)
        .map_err(|_| format!("安装包地址无效：{}", asset.file_name))?;
    if url.scheme() != "https" {
        return Err(format!("安装包地址必须使用 HTTPS：{}", asset.file_name));
    }
    if asset.sha256.len() != 64 || !asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("安装包 SHA-256 无效：{}", asset.file_name));
    }
    Ok(())
}

pub(super) fn current_update_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        std::env::consts::OS
    }
}

pub(super) fn current_update_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        std::env::consts::ARCH
    }
}

fn installable_package_priority(asset: &UpdateManifestAsset) -> Option<u8> {
    match (
        current_update_platform(),
        asset.package_type.trim().to_ascii_lowercase().as_str(),
    ) {
        ("windows", "nsis") => Some(0),
        ("macos", "app-zip") => Some(0),
        _ => None,
    }
}

fn selected_update_asset(assets: &[UpdateManifestAsset]) -> Option<UpdateManifestAsset> {
    let platform = current_update_platform();
    let arch = current_update_arch();
    assets
        .iter()
        .filter(|asset| {
            asset.platform.eq_ignore_ascii_case(platform)
                && asset.arch.eq_ignore_ascii_case(arch)
                && installable_package_priority(asset).is_some()
        })
        .min_by_key(|asset| installable_package_priority(asset).unwrap_or(u8::MAX))
        .cloned()
}

fn asset_info(asset: &UpdateManifestAsset) -> UpdateAssetInfo {
    UpdateAssetInfo {
        platform: asset.platform.clone(),
        arch: asset.arch.clone(),
        package_type: asset.package_type.clone(),
        file_name: asset.file_name.clone(),
        url: asset.url.clone(),
        sha256: asset.sha256.clone(),
        size: asset.size,
    }
}

fn update_download_dir(store: &ConfigStore) -> Result<PathBuf, String> {
    let parent = store
        .path()
        .parent()
        .ok_or_else(|| "Codey 配置路径无父目录，无法创建更新缓存".to_string())?;
    Ok(parent.join("updates"))
}

async fn download_update_asset(
    client: &reqwest::Client,
    store: &ConfigStore,
    version: &str,
    asset: &UpdateAssetInfo,
) -> Result<PathBuf, String> {
    validate_update_asset_info(asset)?;
    let url = reqwest::Url::parse(&asset.url)
        .map_err(|_| format!("安装包地址无效：{}", asset.file_name))?;
    let directory = update_download_dir(store)?.join(format!("v{version}"));
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|error| format!("创建更新缓存目录失败：{error}"))?;
    let destination = directory.join(&asset.file_name);
    let temporary = directory.join(format!(".{}.download", asset.file_name));
    let _ = tokio::fs::remove_file(&temporary).await;

    let response = client
        .get(url)
        .header(
            USER_AGENT,
            format!("Codey/{} update-download", env!("CARGO_PKG_VERSION")),
        )
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .map_err(|error| format!("下载更新失败：{error}"))?
        .error_for_status()
        .map_err(|error| format!("下载安装包失败：{error}"))?;
    if response.url().scheme() != "https" {
        return Err("安装包地址重定向到了非 HTTPS 地址".to_string());
    }

    let verified_download = async {
        let mut file = tokio::fs::File::create(&temporary)
            .await
            .map_err(|error| format!("创建更新缓存文件失败：{error}"))?;
        let mut stream = response.bytes_stream();
        let mut hasher = Sha256::new();
        let mut bytes_written = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| format!("读取更新下载数据失败：{error}"))?;
            bytes_written += chunk.len() as u64;
            if bytes_written > asset.size {
                return Err("下载的安装包大小超过更新清单声明".to_string());
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|error| format!("写入更新缓存文件失败：{error}"))?;
        }
        file.flush()
            .await
            .map_err(|error| format!("刷新更新缓存文件失败：{error}"))?;
        drop(file);

        if bytes_written != asset.size {
            return Err(format!(
                "安装包大小不一致：期望 {} 字节，实际 {} 字节",
                asset.size, bytes_written
            ));
        }
        let actual_sha256 = format!("{:x}", hasher.finalize());
        if !actual_sha256.eq_ignore_ascii_case(&asset.sha256) {
            return Err("安装包 SHA-256 校验失败".to_string());
        }
        Ok(())
    }
    .await;
    if let Err(error) = verified_download {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    let _ = tokio::fs::remove_file(&destination).await;
    if let Err(error) = tokio::fs::rename(&temporary, &destination).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(format!("保存更新安装包失败：{error}"));
    }
    Ok(destination)
}

fn validate_update_asset_info(asset: &UpdateAssetInfo) -> Result<(), String> {
    if asset.platform.trim().is_empty()
        || asset.arch.trim().is_empty()
        || asset.package_type.trim().is_empty()
        || asset.file_name.trim().is_empty()
        || asset.size == 0
    {
        return Err("更新清单包含不完整的安装包信息".to_string());
    }
    if asset.file_name.contains(['/', '\\'])
        || Path::new(&asset.file_name).components().count() != 1
    {
        return Err(format!("安装包文件名无效：{}", asset.file_name));
    }
    let url = reqwest::Url::parse(&asset.url)
        .map_err(|_| format!("安装包地址无效：{}", asset.file_name))?;
    if url.scheme() != "https" {
        return Err(format!("安装包地址必须使用 HTTPS：{}", asset.file_name));
    }
    if asset.sha256.len() != 64 || !asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("安装包 SHA-256 无效：{}", asset.file_name));
    }
    Ok(())
}

fn validate_downloaded_update_path(
    store: &ConfigStore,
    file_path: &str,
) -> Result<PathBuf, String> {
    let path = PathBuf::from(file_path);
    if !path.is_absolute() {
        return Err("更新安装包路径必须是绝对路径".to_string());
    }
    let root = update_download_dir(store)?
        .canonicalize()
        .map_err(|error| format!("读取更新缓存目录失败：{error}"))?;
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("读取更新安装包失败：{error}"))?;
    if !canonical.starts_with(&root) {
        return Err("只能安装 Codey 下载缓存中的更新包".to_string());
    }
    if !canonical.is_file() {
        return Err("更新安装包路径必须指向文件".to_string());
    }
    Ok(canonical)
}

#[cfg(target_os = "windows")]
fn spawn_update_installer(update_path: &Path) -> Result<(), String> {
    crate::update_helper::spawn_update_installer(update_path)
}

#[cfg(target_os = "macos")]
fn spawn_update_installer(update_path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    if !update_path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        return Err("macOS 更新安装包必须是 .zip".to_string());
    }
    let app_bundle = current_macos_app_bundle()
        .ok_or_else(|| "当前 Codey 不是从 .app 包运行，无法自动替换".to_string())?;
    let script_path = update_path
        .parent()
        .ok_or_else(|| "更新安装包路径无父目录".to_string())?
        .join("install-codey-update.sh");
    fs::write(
        &script_path,
        r#"#!/bin/sh
set -eu
parent_pid="$1"
archive="$2"
app_bundle="$3"
app_parent="$(dirname "$app_bundle")"
app_name="$(basename "$app_bundle")"
while kill -0 "$parent_pid" 2>/dev/null; do
  sleep 0.2
done
tmp_dir="$app_parent/.${app_name}.codey-update.$$"
rm -rf "$tmp_dir"
mkdir -p "$tmp_dir"
/usr/bin/ditto -x -k "$archive" "$tmp_dir"
test -d "$tmp_dir/$app_name"
rm -rf "$app_bundle"
mv "$tmp_dir/$app_name" "$app_bundle"
rm -rf "$tmp_dir"
/usr/bin/open "$app_bundle"
"#,
    )
    .map_err(|error| format!("写入更新安装脚本失败：{error}"))?;
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("设置更新安装脚本权限失败：{error}"))?;
    std::process::Command::new("/bin/sh")
        .arg(&script_path)
        .arg(std::process::id().to_string())
        .arg(update_path)
        .arg(app_bundle)
        .spawn()
        .map_err(|error| format!("启动更新安装脚本失败：{error}"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn current_macos_app_bundle() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .ancestors()
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("app"))
        .map(Path::to_path_buf)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn spawn_update_installer(_update_path: &Path) -> Result<(), String> {
    Err("当前平台暂不支持自动安装更新".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_asset() -> UpdateManifestAsset {
        UpdateManifestAsset {
            platform: current_update_platform().to_string(),
            arch: current_update_arch().to_string(),
            package_type: if cfg!(target_os = "windows") {
                "nsis"
            } else if cfg!(target_os = "macos") {
                "app-zip"
            } else {
                "unsupported"
            }
            .to_string(),
            file_name: "codey-update.pkg".to_string(),
            url: "https://updates.example.test/codey-update.pkg".to_string(),
            sha256: "a".repeat(64),
            size: 42,
        }
    }

    fn valid_manifest(version: &str) -> UpdateManifest {
        UpdateManifest {
            schema_version: 1,
            version: version.to_string(),
            tag: format!("v{version}"),
            assets: vec![valid_asset()],
        }
    }

    #[test]
    fn update_manifest_validation_rejects_invalid_security_fields() {
        let mut manifest = valid_manifest("2.0.0");
        assert!(
            assess_update_manifest("1.0.0", &manifest)
                .unwrap()
                .update_available
        );

        manifest.tag = "v1.0.0".to_string();
        assert!(
            assess_update_manifest("1.0.0", &manifest)
                .unwrap_err()
                .contains("版本和标签不一致")
        );

        manifest = valid_manifest("2.0.0");
        manifest.assets[0].sha256 = "not-a-sha".to_string();
        assert!(
            assess_update_manifest("1.0.0", &manifest)
                .unwrap_err()
                .contains("SHA-256")
        );

        manifest = valid_manifest("2.0.0");
        manifest.assets[0].file_name = "../escape.pkg".to_string();
        assert!(
            assess_update_manifest("1.0.0", &manifest)
                .unwrap_err()
                .contains("文件名无效")
        );
    }

    #[test]
    fn update_manifest_equal_version_is_not_available() {
        let check = assess_update_manifest("2.0.0", &valid_manifest("2.0.0")).unwrap();

        assert!(!check.update_available);
        assert_eq!(check.current_version, "2.0.0");
        assert_eq!(check.latest_version, "2.0.0");
    }

    #[test]
    fn downloaded_update_path_must_be_a_file_inside_the_cache() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("config.json"));
        let update_root = directory.path().join("updates");
        let version_dir = update_root.join("v2.0.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        let update = version_dir.join("codey-update.pkg");
        std::fs::write(&update, b"verified").unwrap();
        let outside = directory.path().join("outside.pkg");
        std::fs::write(&outside, b"outside").unwrap();

        assert_eq!(
            validate_downloaded_update_path(&store, update.to_str().unwrap()).unwrap(),
            update.canonicalize().unwrap()
        );
        assert!(
            validate_downloaded_update_path(&store, "relative.pkg")
                .unwrap_err()
                .contains("绝对路径")
        );
        assert!(
            validate_downloaded_update_path(&store, outside.to_str().unwrap())
                .unwrap_err()
                .contains("下载缓存")
        );
        assert!(
            validate_downloaded_update_path(&store, version_dir.to_str().unwrap())
                .unwrap_err()
                .contains("必须指向文件")
        );
    }

    #[cfg(unix)]
    #[test]
    fn downloaded_update_path_rejects_symlinks_that_escape_the_cache() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("config.json"));
        let update_root = directory.path().join("updates");
        std::fs::create_dir_all(&update_root).unwrap();
        let outside = directory.path().join("outside.pkg");
        std::fs::write(&outside, b"outside").unwrap();
        let link = update_root.join("linked.pkg");
        symlink(&outside, &link).unwrap();

        assert!(
            validate_downloaded_update_path(&store, link.to_str().unwrap())
                .unwrap_err()
                .contains("下载缓存")
        );
    }
}
