use super::Manifest;
use serde::Serialize;
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
}

pub struct Package {
    pub inspection: Inspection,
    pub files: BTreeMap<String, Vec<u8>>,
    pub default_config: String,
}

pub fn digest(bytes: &[u8]) -> String {
    crate::fs_util::sha256_hex(bytes)
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
    if !safe_relative(&manifest.entry) {
        return Err("插件入口路径无效".into());
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
    let default_config = if manifest
        .legacy_config_schema
        .as_ref()
        .and_then(serde_json::Value::as_str)
        == Some(super::CONFIG_FILE)
    {
        "{}\n".to_owned()
    } else if let Some(bytes) = files.get(super::CONFIG_FILE) {
        let content = std::str::from_utf8(bytes).map_err(|_| "配置文件必须为 UTF-8")?;
        super::parse_config(content)?;
        content.to_owned()
    } else {
        "{}\n".to_owned()
    };
    Ok(Package {
        inspection: Inspection {
            path: path.to_string_lossy().into(),
            sha256,
            manifest,
        },
        files,
        default_config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn package_fixture(
        config: Option<&[u8]>,
        legacy_schema: Option<&str>,
    ) -> Result<Package, String> {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture.codey-plugin");
        let mut manifest =
            serde_json::to_value(super::super::tests::fixture_package().inspection.manifest)
                .unwrap();
        manifest["librarySha256"] = digest(b"library").into();
        if let Some(schema) = legacy_schema {
            manifest["configSchema"] = schema.into();
            manifest["configUi"] =
                serde_json::json!({"type":"html", "entry":"missing.html", "sha256":"ignored"});
        }
        let mut archive = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        let mut entries = vec![
            (
                "manifest.json".to_owned(),
                serde_json::to_vec(&manifest).unwrap(),
            ),
            (
                manifest["entry"].as_str().unwrap().to_owned(),
                b"library".to_vec(),
            ),
        ];
        if let Some(config) = config {
            entries.push(("config.json".into(), config.to_vec()));
        }
        for (name, bytes) in entries {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(&bytes).unwrap();
        }
        archive.finish().unwrap();
        read(&path)
    }

    #[test]
    fn package_defaults_are_fixed_json_text_and_legacy_metadata_is_ignored() {
        let text = b"{\r\n \"value\": 1\r\n}\r\n";
        let package = package_fixture(Some(text), None).unwrap();
        assert_eq!(package.default_config.as_bytes(), text);
        let legacy = package_fixture(Some(b"not a schema"), Some("config.json")).unwrap();
        assert_eq!(legacy.default_config.trim(), "{}");
        let legacy_without_file = package_fixture(None, Some("missing.schema.json")).unwrap();
        assert_eq!(legacy_without_file.default_config.trim(), "{}");
        let metadata = serde_json::to_value(legacy.inspection).unwrap();
        assert!(metadata.get("configSchema").is_none());
        assert!(metadata["manifest"].get("configSchema").is_none());
        assert!(metadata["manifest"].get("configUi").is_none());
        for invalid in [b"[]".as_slice(), b"{broken".as_slice(), &[0xff]] {
            assert!(package_fixture(Some(invalid), None).is_err());
        }
        assert!(
            package_fixture(
                Some(&vec![b' '; super::super::MAX_CONFIG_BYTES as usize + 1]),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn package_validates_comments_and_preserves_the_default_text() {
        let text = "{\r\n  \"_comments\": {\"value\": \"说明\"},\r\n  \"value\": 1, \"rules\": [{\"_comments\": {}}]\r\n}\r\n";
        let package = package_fixture(Some(text.as_bytes()), None).unwrap();
        assert_eq!(package.default_config, text);
        assert_eq!(package.files["config.json"], text.as_bytes());
        for invalid in [
            r#"{"_comments":[]}"#,
            r#"{"rules":[{"_comments":{"model":false}}]}"#,
        ] {
            let error = package_fixture(Some(invalid.as_bytes()), None)
                .err()
                .unwrap();
            assert!(error.contains("_comments"), "{error}");
        }
    }

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
