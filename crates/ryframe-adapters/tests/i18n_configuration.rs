use std::{collections::BTreeMap, fs, path::Path};

use ryframe_adapters::i18n::{I18nError, LocalizerLoader};
use ryframe_kernel::{Locale, Localizer};

fn write_resources(directory: &Path) {
    fs::create_dir_all(directory).expect("创建隔离资源目录");
    fs::write(
        directory.join("zh-CN.toml"),
        "[common]\nsuccess = \"配置目录成功\"\n[user]\nwelcome = \"你好 {name}\"\n",
    )
    .expect("写入中文资源");
    fs::write(
        directory.join("en-US.toml"),
        "[common]\nsuccess = \"Configured success\"\n[user]\nwelcome = \"Hello {name}\"\n",
    )
    .expect("写入英文资源");
}

fn assert_embedded(localizer: &Localizer) {
    let embedded = LocalizerLoader::embedded().expect("原内嵌资源必须有效");
    for locale in Locale::ALL {
        assert_eq!(
            localizer.translate(locale, "common.success"),
            embedded.translate(locale, "common.success")
        );
        let arguments = BTreeMap::from([("name".into(), "Alice".into())]);
        assert_eq!(
            localizer.translate_with_args(locale, "user.welcome", &arguments),
            embedded.translate_with_args(locale, "user.welcome", &arguments)
        );
    }
}

#[test]
fn configured_directory_loads_both_languages_without_falling_back() {
    let root = tempfile::tempdir().expect("创建隔离临时目录");
    let directory = root.path().join("语言 资源");
    write_resources(&directory);
    for strict in [false, true] {
        let localizer = LocalizerLoader::load_configured(&directory, strict)
            .expect("明确配置的完整资源必须加载成功");
        assert_eq!(
            localizer.translate(Locale::ZhCn, "common.success"),
            "配置目录成功"
        );
        assert_eq!(
            localizer.translate(Locale::EnUs, "common.success"),
            "Configured success"
        );
        let arguments = BTreeMap::from([("name".into(), "Alice".into())]);
        assert_eq!(
            localizer.translate_with_args(Locale::ZhCn, "user.welcome", &arguments),
            "你好 Alice"
        );
        assert_eq!(
            localizer.translate_with_args(Locale::EnUs, "user.welcome", &arguments),
            "Hello Alice"
        );
    }
}

#[test]
fn missing_directory_fails_strictly_and_otherwise_uses_original_embedded_resources() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("未创建目录");
    assert!(matches!(
        LocalizerLoader::load_configured(&missing, true),
        Err(I18nError::ReadDirectory { .. })
    ));
    assert_embedded(&LocalizerLoader::load_configured(&missing, false).unwrap());
}

#[test]
fn either_missing_language_fails_strictly_and_falls_back_as_a_whole() {
    for locale in Locale::ALL {
        let root = tempfile::tempdir().unwrap();
        write_resources(root.path());
        fs::remove_file(root.path().join(format!("{}.toml", locale.as_str()))).unwrap();
        assert!(matches!(
            LocalizerLoader::load_configured(root.path(), true),
            Err(I18nError::MissingResource { .. })
        ));
        assert_embedded(&LocalizerLoader::load_configured(root.path(), false).unwrap());
    }
}

#[test]
fn malformed_toml_fails_strictly_and_otherwise_falls_back() {
    for locale in Locale::ALL {
        let root = tempfile::tempdir().unwrap();
        write_resources(root.path());
        fs::write(
            root.path().join(format!("{}.toml", locale.as_str())),
            "[broken",
        )
        .unwrap();
        assert!(matches!(
            LocalizerLoader::load_configured(root.path(), true),
            Err(I18nError::ParseFile { .. })
        ));
        assert_embedded(&LocalizerLoader::load_configured(root.path(), false).unwrap());
    }
}

#[test]
fn resource_shape_and_placeholder_errors_keep_the_same_strict_and_fallback_policy() {
    for source in [
        "[common]\nsuccess = 7\n",
        "[common]\nother = \"missing matching keys\"\n",
        "[common]\nsuccess = \"Configured success\"\n[user]\nwelcome = \"Hello {other}\"\n",
    ] {
        let root = tempfile::tempdir().unwrap();
        write_resources(root.path());
        fs::write(root.path().join("en-US.toml"), source).unwrap();
        let error = LocalizerLoader::load_configured(root.path(), true)
            .expect_err("无效资源不得在严格模式下静默回退");
        assert!(matches!(
            error,
            I18nError::NonStringValue { .. } | I18nError::InvalidResources(_)
        ));
        assert_embedded(&LocalizerLoader::load_configured(root.path(), false).unwrap());
    }
}

#[test]
fn a_file_instead_of_the_directory_obeys_the_same_loading_policy() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("资源文件");
    fs::write(&file, "not a directory").unwrap();
    assert!(matches!(
        LocalizerLoader::load_configured(&file, true),
        Err(I18nError::ReadDirectory { .. })
    ));
    assert_embedded(&LocalizerLoader::load_configured(&file, false).unwrap());
}
