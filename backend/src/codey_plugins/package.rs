use super::{Manifest, schema};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{Cursor, Read},
    path::Path,
};

pub const MAX_PACKAGE: u64 = 64 * 1024 * 1024;
const MAX_EXTRACTED: u64 = 128 * 1024 * 1024;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub path: String,
    pub sha256: String,
    pub manifest: Manifest,
    pub config_schema: Value,
}

pub struct Package {
    pub inspection: Inspection,
    pub files: BTreeMap<String, Vec<u8>>,
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 240
        && !path.starts_with('/')
        && !path.contains('\\')
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with('.')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
                && ![
                    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
                    "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7",
                    "LPT8", "LPT9",
                ]
                .contains(
                    &part
                        .split('.')
                        .next()
                        .unwrap_or("")
                        .to_ascii_uppercase()
                        .as_str(),
                )
        })
}

pub fn valid_id(id: &str) -> bool {
    id.len() <= 96
        && id.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        && !id.contains("..")
        && !id.ends_with('.')
}

pub fn validate_manifest(manifest: &Manifest) -> Result<(), String> {
    if !valid_id(&manifest.id) {
        return Err("插件 ID 无效".into());
    }
    if manifest.name.trim().is_empty() || manifest.name.len() > 160 {
        return Err("插件名称无效".into());
    }
    if manifest.version.len() > 64
        || !safe_relative(&manifest.version)
        || manifest.version.contains('/')
    {
        return Err("插件版本无效".into());
    }
    semver::Version::parse(&manifest.version).map_err(|e| format!("插件版本必须是 semver: {e}"))?;
    if manifest.abi_version != codey_plugin_sdk::ABI_VERSION {
        return Err("不支持该插件 ABI 版本".into());
    }
    if manifest.platform != std::env::consts::OS || manifest.arch != std::env::consts::ARCH {
        return Err(format!(
            "插件平台不匹配，当前为 {} / {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }
    if !safe_relative(&manifest.entry) || !safe_relative(&manifest.config_schema) {
        return Err("插件入口或配置路径无效".into());
    }
    let extension = if cfg!(target_os = "windows") {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    if Path::new(&manifest.entry)
        .extension()
        .and_then(|v| v.to_str())
        != Some(extension)
    {
        return Err("插件动态库扩展名不匹配".into());
    }
    if manifest.library_sha256.len() != 64
        || !manifest
            .library_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err("librarySha256 必须是小写 SHA-256".into());
    }
    if manifest
        .capabilities
        .iter()
        .any(|s| s != "request.beforeSend")
    {
        return Err("插件声明了尚未支持的扩展能力".into());
    }
    if manifest
        .header_names
        .iter()
        .any(|s| !super::allowed_header_name(s))
    {
        return Err("插件声明了禁止修改或无效的请求头".into());
    }
    if !manifest.header_names.is_empty()
        && !manifest
            .capabilities
            .iter()
            .any(|s| s == "request.beforeSend")
    {
        return Err("headerNames 需要 request.beforeSend 能力".into());
    }
    Ok(())
}

pub fn read(path: &Path) -> Result<Package, String> {
    if path.extension().and_then(|s| s.to_str()) != Some("codey-plugin") {
        return Err("请选择 .codey-plugin 安装包".into());
    }
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_PACKAGE {
        return Err("插件包不是普通文件或超过 64 MiB".into());
    }
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_PACKAGE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_PACKAGE {
        return Err("插件包超过大小限制".into());
    }
    let sha256 = digest(&bytes);
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("ZIP 格式无效: {e}"))?;
    if zip.len() > 256 {
        return Err("插件包条目超过 256 个".into());
    }
    let mut files = BTreeMap::new();
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let is_dir = entry.is_dir();
        let name = entry
            .name()
            .strip_suffix('/')
            .unwrap_or(entry.name())
            .to_owned();
        if !safe_relative(&name) || !seen.insert(name.to_ascii_lowercase()) {
            return Err("插件包存在非法路径或重复条目".into());
        }
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170000;
            if kind != 0 && kind != 0o100000 && !(kind == 0o040000 && is_dir) {
                return Err("插件包不允许符号链接或特殊文件".into());
            }
        }
        if is_dir {
            continue;
        }
        total = total.checked_add(entry.size()).ok_or("插件包大小溢出")?;
        if total > MAX_EXTRACTED || entry.size() > MAX_PACKAGE {
            return Err("插件解压大小超过限制".into());
        }
        let mut content = Vec::new();
        (&mut entry)
            .take(MAX_PACKAGE + 1)
            .read_to_end(&mut content)
            .map_err(|e| e.to_string())?;
        if content.len() as u64 != entry.size() || content.len() as u64 > MAX_PACKAGE {
            return Err("插件条目大小不一致".into());
        }
        files.insert(name, content);
    }
    for name in files.keys() {
        let parts: Vec<_> = name.split('/').collect();
        for i in 1..parts.len() {
            if files
                .keys()
                .any(|p| p.eq_ignore_ascii_case(&parts[..i].join("/")))
            {
                return Err("插件包文件与目录冲突".into());
            }
        }
    }
    let manifest_bytes = files.get("manifest.json").ok_or("缺少 manifest.json")?;
    if manifest_bytes.len() > 1024 * 1024 {
        return Err("manifest 过大".into());
    }
    let manifest: Manifest =
        serde_json::from_slice(manifest_bytes).map_err(|e| format!("manifest 无效: {e}"))?;
    validate_manifest(&manifest)?;
    let library = files.get(&manifest.entry).ok_or("找不到插件入口动态库")?;
    if digest(library) != manifest.library_sha256 {
        return Err("插件动态库 SHA-256 不匹配".into());
    }
    let config_bytes = files.get(&manifest.config_schema).ok_or("找不到配置定义")?;
    if config_bytes.len() > 1024 * 1024 {
        return Err("配置定义过大".into());
    }
    let config_schema: Value = serde_json::from_slice(config_bytes).map_err(|e| e.to_string())?;
    schema::check_schema(&config_schema)?;
    if config_schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err("插件根配置必须为 object".into());
    }
    Ok(Package {
        inspection: Inspection {
            path: path.to_string_lossy().into(),
            sha256,
            manifest,
            config_schema,
        },
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_traversal_and_platform_path_aliases() {
        for path in [
            "../a",
            "/a",
            "a/../b",
            "a\\b",
            "C:/a",
            "a//b",
            "a/CON.txt",
            "a.",
            "a/./b",
        ] {
            assert!(!safe_relative(path), "{path}");
        }
        assert!(safe_relative("lib/plugin.dylib"));
    }
}
