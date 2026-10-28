use super::*;

fn fixture() -> tempfile::TempDir {
    let base = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("dataset-authority-unit-")
        .tempdir_in(base)
        .unwrap()
}

#[test]
fn delegated_json_read_remains_protected_after_reader_returns() {
    let directory = fixture();
    let file = directory.path().join("input.json");
    std::fs::write(&file, b"{\"value\":1}").unwrap();
    let window = reads::Window::begin().unwrap();
    strict::read(&file).unwrap();
    let changed = std::fs::write(&file, b"{\"value\":2}");
    let result = window.finish(Ok(()));
    if changed.is_ok() {
        assert!(result.is_err());
    } else {
        assert!(result.is_ok());
    }
}

#[test]
fn optional_configuration_appearance_and_directory_additions_are_rejected() {
    let directory = fixture();
    let optional = directory.path().join("tools.toml");
    let window = reads::Window::begin().unwrap();
    reads::optional(&optional).unwrap();
    std::fs::write(&optional, b"[verification]").unwrap();
    assert!(window.finish(Ok(())).is_err());
    drop(window);
    let window = reads::Window::begin().unwrap();
    reads::directory(directory.path()).unwrap();
    std::fs::write(directory.path().join("unexpected.json"), b"{}").unwrap();
    assert!(window.finish(Ok(())).is_err());
}

#[test]
fn explicit_interpreter_is_required_without_falling_back_to_path() {
    let directory = fixture();
    let mut inputs = inputs::Inputs::new(directory.path());
    let error = commands::interpreter(directory.path(), &mut inputs)
        .unwrap_err()
        .to_string();
    assert!(error.contains("verification.python_program"));
}

#[test]
fn interpreter_drift_is_rejected_even_when_command_is_otherwise_unchanged() {
    let directory = fixture();
    let expected = directory.path().join("expected.exe");
    let different = directory.path().join("different.exe");
    std::fs::write(&expected, b"fixture-a").unwrap();
    std::fs::write(&different, b"fixture-b").unwrap();
    let expected = json!({"api":[expected,"--flag"],"worker":["unused"],"frontend":["unused"]});
    let mut actual = expected.clone();
    actual["api"][0] = json!(different);
    assert!(commands::recorded_spelling(expected, &actual).is_err());
}

#[test]
fn unknown_arguments_and_duplicate_fields_cannot_open_a_read_window() {
    assert!(
        request(&json!({"backend_dir":"x","runtime_receipt":"x","target_plan":"x","extra":true}))
            .is_err()
    );
    assert!(
        strict::parse_fragment(r#"{"backend_dir":"a","backend_dir":"b"}"#, "恢复参数").is_err()
    );
    let window = reads::Window::begin().unwrap();
    assert!(window.finish(Ok(())).is_ok());
}
