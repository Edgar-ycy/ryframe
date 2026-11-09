use super::*;

fn denied() -> Box<dyn Error> {
    Box::new(AccessDenied {
        pid: 42,
        operation: "读取映像",
    })
}

#[derive(Debug)]
struct Wrapped(Box<dyn Error>);
impl fmt::Display for Wrapped {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("外层错误")
    }
}
impl Error for Wrapped {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[test]
fn permission_classification_uses_types_and_sources_instead_of_display() {
    let marker = denied();
    assert!(is_access_denied(marker.as_ref()));
    assert_eq!(
        marker.to_string(),
        "进程 42 的读取映像被拒绝访问（错误码5）"
    );
    let same_text: Box<dyn Error> = marker.to_string().into();
    assert!(!is_access_denied(same_text.as_ref()));
    assert!(is_access_denied(&Wrapped(marker)));
    let unknown = std::io::Error::other("错误码5");
    assert!(!is_access_denied(&unknown));
    assert!(!is_access_denied(&std::io::Error::from_raw_os_error(5)));
}

#[test]
fn composite_keeps_both_original_errors_and_primary_source() {
    for denied_primary in [true, false] {
        let (observation, wait): (Box<dyn Error>, Box<dyn Error>) = if denied_primary {
            (denied(), "等待失败".into())
        } else {
            ("观察失败".into(), denied())
        };
        let primary = observation.to_string();
        let expected = format!("{observation}；同一进程句柄的退出状态也无法核验：{wait}");
        let failure = ObservationFailure { observation, wait };
        assert_eq!(failure.to_string(), expected);
        assert_eq!(failure.source().unwrap().to_string(), primary);
        assert!(is_access_denied(&failure));
        assert!(is_access_denied(&Wrapped(Box::new(failure))));
    }
    assert!(!is_access_denied(&ObservationFailure {
        observation: "错误码5".into(),
        wait: "拒绝访问".into(),
    }));
}

#[cfg(windows)]
#[test]
fn windows_error_mapping_and_exit_race_preserve_permission_type() {
    for operation in [
        "打开进程",
        "读取创建时间",
        "读取映像",
        "首次等待退出",
        "确认等待退出",
    ] {
        assert!(is_access_denied(
            windows_error(42, operation, winsafe::co::ERROR::ACCESS_DENIED).as_ref()
        ));
        assert!(!is_access_denied(
            windows_error(42, operation, winsafe::co::ERROR::INVALID_PARAMETER).as_ref()
        ));
    }
    assert_eq!(
        confirm_live_observation::<u8>(Err(denied()), Ok(true)).unwrap(),
        None
    );
    let failed = confirm_live_observation::<u8>(Err("观察失败".into()), Err(denied())).unwrap_err();
    assert!(is_access_denied(failed.as_ref()));
    let failed = confirm_live_observation::<u8>(Err(denied()), Err("等待失败".into())).unwrap_err();
    assert!(is_access_denied(failed.as_ref()));
}
