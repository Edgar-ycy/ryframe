use std::path::PathBuf;

use ryframe_config::ConfigurationArgs;
use ryframe_kernel::AppError;

fn arguments(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn extracts_directory_at_each_command_position_without_reordering_arguments() {
    for values in [
        vec!["--config-dir", "自定义 目录", "control", "verify", "--plan"],
        vec!["control", "--config-dir", "自定义 目录", "verify", "--plan"],
        vec!["control", "verify", "--plan", "--config-dir", "自定义 目录"],
    ] {
        let parsed = ConfigurationArgs::parse(arguments(&values)).unwrap();
        assert_eq!(parsed.directory, PathBuf::from("自定义 目录"));
        assert_eq!(
            parsed.arguments,
            arguments(&["control", "verify", "--plan"])
        );
    }
}

#[test]
fn defaults_to_config_and_leaves_unknown_options_for_the_command_parser() {
    let parsed = ConfigurationArgs::parse(Vec::new()).unwrap();
    assert_eq!(parsed.directory, PathBuf::from("config"));
    assert!(parsed.arguments.is_empty());
    let original = arguments(&[
        "--unknown",
        "value",
        "--config-directory",
        "other",
        "--plan",
    ]);
    let parsed = ConfigurationArgs::parse(original.clone()).unwrap();
    assert_eq!(parsed.directory, PathBuf::from("config"));
    assert_eq!(parsed.arguments, original);
}

#[test]
fn delimiter_preserves_every_following_argument_without_interpreting_it() {
    let suffix = [
        "--",
        "--config-dir",
        "",
        "--config-dir=literal",
        "--config-dir",
        "again",
    ];
    let parsed = ConfigurationArgs::parse(arguments(&suffix)).unwrap();
    assert_eq!(parsed.directory, PathBuf::from("config"));
    assert_eq!(parsed.arguments, arguments(&suffix));
    let mut original = arguments(&["--config-dir", "selected", "control"]);
    original.extend(arguments(&suffix));
    let parsed = ConfigurationArgs::parse(original).unwrap();
    let mut expected = arguments(&["control"]);
    expected.extend(arguments(&suffix));
    assert_eq!(parsed.directory, PathBuf::from("selected"));
    assert_eq!(parsed.arguments, expected);
}

#[test]
fn rejects_duplicate_missing_empty_control_characters_and_equals_syntax_without_values() {
    for values in [
        vec![
            "--config-dir",
            "private-first",
            "--config-dir",
            "private-second",
        ],
        vec!["--config-dir", "--config-dir"],
        vec!["--config-dir"],
        vec!["--config-dir", "--plan"],
        vec!["--config-dir", "--"],
        vec!["--config-dir", ""],
        vec!["--config-dir", "   "],
        vec!["--config-dir", "private-\0path"],
        vec!["--config-dir", "private-\rpath"],
        vec!["--config-dir", "private-\npath"],
        vec!["--config-dir=private-equals"],
    ] {
        let AppError::Config(message) = ConfigurationArgs::parse(arguments(&values)).unwrap_err()
        else {
            panic!("应返回配置参数错误");
        };
        assert!(message.contains("--config-dir"));
        assert!(!message.contains("private-"));
        assert!(!message.contains('\0'));
        assert!(!message.contains('\r'));
        assert!(!message.contains('\n'));
    }
    let error = ConfigurationArgs::parse(arguments(&["--config-dir=private-equals"])).unwrap_err();
    let AppError::Config(message) = error else {
        panic!("应返回配置参数错误")
    };
    assert!(message.contains("--config-dir <目录>"));
}

#[test]
fn directory_need_not_exist_and_paths_with_spaces_and_chinese_are_preserved() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.local-tests/configuration-args-uncreated")
        .join(format!("不存在的目录 {}", std::process::id()));
    assert!(!directory.exists());
    let parsed = ConfigurationArgs::parse(vec![
        "--config-dir".into(),
        directory.to_str().unwrap().to_owned(),
        "--plan".into(),
    ])
    .unwrap();
    assert_eq!(parsed.directory, directory);
    assert_eq!(parsed.arguments, arguments(&["--plan"]));
}
