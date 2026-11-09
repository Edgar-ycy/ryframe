use super::*;

#[test]
fn stage_request_and_state_boundaries_reject_wrong_or_replayed_attempts() {
    let backend = crate::workspace::root_dir();
    let runner = SourceRegisterRunner::new(&backend);
    let mut request = StageRequest {
        backend: &backend,
        run_dir: &backend,
        stage: "seed-runtime",
        mode: "source-register",
        attempt: None,
    };
    assert_eq!(runner.request(&request, false).unwrap(), None);
    for (stage, mode, attempt) in [
        ("copy", "source-register", None),
        ("seed-runtime", "close", None),
        ("seed-runtime", "source-register", Some(1)),
    ] {
        request.stage = stage;
        request.mode = mode;
        request.attempt = attempt;
        assert!(runner.request(&request, false).is_err());
    }
    request.stage = "seed-runtime";
    request.mode = "source-register";
    request.attempt = Some(0);
    assert!(runner.request(&request, true).is_err());
    let saved = json!({"attempts":[{"number":1,"stage":"seed-runtime","mode":"source-register","status":"running"}]});
    assert!(require_state(&saved, Some(1)).is_ok());
    assert!(require_state(&saved, None).is_err());
    let published = json!({"attempts":[{"number":1,"stage":"seed-runtime","mode":"source-register","status":"passed"}]});
    assert!(require_state(&published, None).is_err());
    assert!(require_state(&json!({"attempts":[]}), None).is_ok());
    let mut duplicate = saved.clone();
    duplicate["attempts"]
        .as_array_mut()
        .unwrap()
        .push(saved["attempts"][0].clone());
    assert!(require_state(&duplicate, Some(1)).is_err());
}

#[test]
fn owner_model_binds_manifest_directory_and_complete_native_identity() {
    let run = crate::workspace::root_dir().join(".local-tests/runner-model");
    let current = json!({"pid":42,"started":"123","executable":"native-fixture"});
    let sha = "a".repeat(64);
    let owner = json!({"format_version":1,"identity":current,"directory":manifest::path_text(&run,"fixture").unwrap(),"manifest_sha256":sha});
    assert!(authorized_owner(&owner, &run, &sha, &current).is_ok());
    for case in [
        "pid",
        "started",
        "executable",
        "manifest",
        "directory",
        "extra",
    ] {
        let mut invalid = owner.clone();
        match case {
            "pid" => invalid["identity"]["pid"] = json!(43),
            "started" => invalid["identity"]["started"] = json!("124"),
            "executable" => invalid["identity"]["executable"] = json!("different"),
            "manifest" => invalid["manifest_sha256"] = json!("b".repeat(64)),
            "directory" => invalid["directory"] = json!("different"),
            _ => invalid["extra"] = json!(true),
        }
        assert!(
            authorized_owner(&invalid, &run, &sha, &current).is_err(),
            "{case}"
        );
    }
}

#[test]
fn invalid_preflight_is_read_only_and_does_not_create_output_or_locks() {
    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("source-runner-preflight-")
        .tempdir_in(local)
        .unwrap();
    let run = directory.path();
    fs::write(run.join("manifest.json"), b"{}").unwrap();
    let before = fs::read(run.join("manifest.json")).unwrap();
    let runner = SourceRegisterRunner::new(&backend);
    let request = StageRequest {
        backend: &backend,
        run_dir: run,
        stage: "seed-runtime",
        mode: "source-register",
        attempt: None,
    };
    assert!(runner.preflight(&request).is_err());
    assert_eq!(fs::read(run.join("manifest.json")).unwrap(), before);
    let entries = fs::read_dir(run)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries, vec![std::ffi::OsString::from("manifest.json")]);
}

#[test]
fn combined_failure_keeps_both_originals_but_never_formats_private_values() {
    let error = preserve::<()>(
        Err("private-primary-config".into()),
        Err("private-release-config".into()),
        "目标 Generation 释放",
    )
    .unwrap_err();
    let failure = error.downcast_ref::<Failure>().unwrap();
    assert_eq!(failure.primary.to_string(), "private-primary-config");
    assert_eq!(
        failure.secondary.as_ref().unwrap().to_string(),
        "private-release-config"
    );
    assert!(!error.to_string().contains("private-"));
    assert!(!format!("{error:?}").contains("private-"));
    assert!(error.to_string().contains("后置操作同时失败"));
    assert!(preserve(Ok(42), Ok(()), "完成").is_ok());
}
