use super::*;

const MIGRATION_FILE: &str = ".codey-locale-migration-v1.json";

#[derive(Deserialize, Serialize)]
struct LocaleMigration {
    version: u32,
}

pub(crate) fn migrate_legacy_default_locale(home: &Path) -> Result<bool> {
    let marker = home.join(MIGRATION_FILE);
    let _lock = RuntimeConfigLock::acquire(&marker)?;
    if let Some(contents) = read_optional(&marker)? {
        let migration: LocaleMigration =
            serde_json::from_slice(&contents).context("读取 Codex 语言迁移记录失败")?;
        if migration.version >= 1 {
            return Ok(false);
        }
    }

    let manager = ConfigManager::new(home.join("config.toml"));
    let snapshot = manager.load()?;
    let mut document = snapshot.document().clone();
    let changed = document
        .get_mut("desktop")
        .and_then(Item::as_table_like_mut)
        .is_some_and(|desktop| {
            if desktop.get("localeOverride").and_then(Item::as_str) != Some("zh-CN") {
                return false;
            }
            desktop.remove("localeOverride");
            true
        });
    if changed {
        manager.replace_document(
            Some(snapshot.revision()),
            document,
            "restore automatic locale detection after removing Codey's default Chinese locale",
            "codex_config.migrate_legacy_default_locale",
        )?;
    }
    atomic_write(
        &marker,
        &serde_json::to_vec(&LocaleMigration { version: 1 })?,
    )
    .context("保存 Codex 语言迁移记录失败")?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_migration_removes_legacy_override_and_preserves_other_settings() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.toml");
        let original = concat!(
            "# User configuration\n",
            "model = \"custom-model\"\n",
            "[desktop]\n",
            "localeOverride = \"zh-CN\"\n",
            "theme = \"dark\" # Keep this preference\n",
            "[model_providers.custom]\n",
            "base_url = \"https://example.invalid/v1\"\n",
        );
        fs::write(&config, original).unwrap();

        assert!(migrate_legacy_default_locale(root.path()).unwrap());
        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            original.replace("localeOverride = \"zh-CN\"\n", "")
        );
        assert_eq!(
            fs::read_to_string(root.path().join("config.toml.bak")).unwrap(),
            original
        );
    }

    #[test]
    fn locale_migration_preserves_other_locales_and_missing_preferences() {
        for contents in [
            "[desktop]\nlocaleOverride = \"en-US\"\n",
            "[desktop]\nlocaleOverride = \"zh-TW\"\n",
            "[desktop]\nlocaleOverride = \"ja-JP\"\n",
            "[desktop]\ntheme = \"dark\"\n",
            "model = \"custom-model\"\n",
            "",
        ] {
            let root = tempfile::tempdir().unwrap();
            let config = root.path().join("config.toml");
            fs::write(&config, contents).unwrap();

            assert!(!migrate_legacy_default_locale(root.path()).unwrap());
            assert_eq!(fs::read_to_string(&config).unwrap(), contents);
            assert!(root.path().join(MIGRATION_FILE).is_file());
        }
    }

    #[test]
    fn locale_migration_preserves_chinese_selected_after_first_launch() {
        for initial in [
            "[desktop]\nlocaleOverride = \"zh-CN\"\n",
            "[desktop]\nlocaleOverride = \"en-US\"\n",
            "",
        ] {
            let root = tempfile::tempdir().unwrap();
            let config = root.path().join("config.toml");
            fs::write(&config, initial).unwrap();
            migrate_legacy_default_locale(root.path()).unwrap();
            let selected = "[desktop]\nlocaleOverride = \"zh-CN\"\n";
            fs::write(&config, selected).unwrap();

            assert!(!migrate_legacy_default_locale(root.path()).unwrap());
            assert_eq!(fs::read_to_string(&config).unwrap(), selected);
        }
    }

    #[test]
    fn locale_migration_supports_inline_desktop_settings() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.toml");
        fs::write(
            &config,
            "desktop = { localeOverride = \"zh-CN\", theme = \"dark\" }\n",
        )
        .unwrap();

        assert!(migrate_legacy_default_locale(root.path()).unwrap());
        let snapshot = ConfigManager::new(&config).load().unwrap();
        let desktop = snapshot.document()["desktop"].as_table_like().unwrap();
        assert!(desktop.get("localeOverride").is_none());
        assert_eq!(desktop.get("theme").and_then(Item::as_str), Some("dark"));
    }

    #[test]
    fn locale_migration_does_not_create_missing_codex_config() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("new-home");

        assert!(!migrate_legacy_default_locale(&home).unwrap());
        assert!(!home.join("config.toml").exists());
        assert!(home.join(MIGRATION_FILE).is_file());
    }

    #[test]
    fn locale_migration_retries_after_invalid_config_is_repaired() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.toml");
        let invalid = "[desktop\nlocaleOverride = \"zh-CN\"\n";
        fs::write(&config, invalid).unwrap();

        assert!(migrate_legacy_default_locale(root.path()).is_err());
        assert!(!root.path().join(MIGRATION_FILE).exists());
        assert_eq!(fs::read_to_string(&config).unwrap(), invalid);
        fs::write(&config, "[desktop]\nlocaleOverride = \"zh-CN\"\n").unwrap();
        assert!(migrate_legacy_default_locale(root.path()).unwrap());
    }

    #[test]
    fn locale_migration_preserves_config_when_marker_is_unreadable() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.toml");
        let original = "[desktop]\nlocaleOverride = \"zh-CN\"\n";
        fs::write(&config, original).unwrap();
        fs::create_dir(root.path().join(MIGRATION_FILE)).unwrap();

        assert!(migrate_legacy_default_locale(root.path()).is_err());
        assert_eq!(fs::read_to_string(&config).unwrap(), original);
        fs::remove_dir(root.path().join(MIGRATION_FILE)).unwrap();
        assert!(migrate_legacy_default_locale(root.path()).unwrap());
    }
}
