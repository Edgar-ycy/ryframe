use super::*;

#[test]
fn response_streams_keep_downloads_bounded_and_binary_media_strict() {
    asynchronous(async {
        crate::http_session::install_crypto_provider().unwrap();
        for (headers, bytes, binary, expected) in [
            (
                "Content-Type: application/octet-stream\r\n",
                20 * 1024 * 1024,
                false,
                Some(20 * 1024 * 1024),
            ),
            (
                "Content-Type: application/json\r\n",
                17 * 1024 * 1024,
                false,
                None,
            ),
            (
                "Content-Type: application/octet-stream\r\n",
                512 * 1024 + 1,
                true,
                None,
            ),
            ("Content-Type: application/JSON\r\n", 10, true, None),
        ] {
            let (address, _, worker) = server(vec![("200 OK", headers, "x".repeat(bytes))]);
            let response = reqwest::Client::new().get(address).send().await.unwrap();
            let result = request::read_response(response, binary).await;
            match expected {
                Some(bytes) => assert_eq!(result.unwrap()["bytes"], bytes),
                None => assert!(result.is_err()),
            }
            worker.join().unwrap();
        }
    });
}

#[test]
fn import_manifest_rejects_duplicate_escaped_foreign_and_changed_samples() {
    use crate::devex::import_samples::{
        ImportSampleManifest, ManifestFile, PerformanceRequest, model, model_sha256,
        performance_username,
    };
    let temp = tempfile::tempdir().unwrap();
    let namespace = "a".repeat(20);
    let content = b"fixture";
    fs::write(temp.path().join("0-0.xlsx"), content).unwrap();
    let request = PerformanceRequest {
        template: temp.path().join("template.xlsx"),
        template_sha256: "b".repeat(64),
        model_sha256: model_sha256().unwrap(),
        directory: temp.path().into(),
        namespace: namespace.clone(),
        concurrency: 1,
        cycles: 1,
    };
    let manifest = ImportSampleManifest {
        format_version: 1,
        namespace: namespace.clone(),
        template_sha256: request.template_sha256.clone(),
        model: model(),
        model_sha256: request.model_sha256.clone(),
        concurrency: 1,
        cycles: 1,
        files: vec![ManifestFile {
            sample: "0:0".into(),
            filename: "0-0.xlsx".into(),
            username: performance_username(&namespace, 0, 0),
            sha256: artifacts::sha(content),
        }],
    };
    let reader = imports::Samples::verify(temp.path(), &manifest, &request).unwrap();
    assert_eq!(reader.read("0:0").unwrap(), content);
    assert!(reader.read("1:0").is_err());
    let mut invalid = manifest.clone();
    invalid.namespace = "c".repeat(20);
    assert!(imports::Samples::verify(temp.path(), &invalid, &request).is_err());
    let mut invalid = manifest.clone();
    invalid.files[0].filename = "../outside.xlsx".into();
    assert!(imports::Samples::verify(temp.path(), &invalid, &request).is_err());
    let mut invalid = manifest.clone();
    invalid.files.push(invalid.files[0].clone());
    assert!(imports::Samples::verify(temp.path(), &invalid, &request).is_err());
    fs::write(temp.path().join("0-0.xlsx"), b"changed").unwrap();
    assert!(reader.read("0:0").is_err());
}

#[test]
fn source_descriptors_and_runner_identity_are_mandatory_before_any_process() {
    let temp = tempfile::tempdir().unwrap();
    let input = input(temp.path());
    let mut config = pool(10);
    let descriptor =
        json!({"path":temp.path().join("receipt.json"),"bytes":1,"sha256":"a".repeat(64)});
    config["contract"]["environment_sha256"] = json!("b".repeat(64));
    config["bindings"]["source_fingerprints"] = json!(input.source_fingerprints);
    config["bindings"]["provenance"] = json!({"python":temp.path().join("python.exe"),"runtime_receipt":descriptor,"target_plan":descriptor,
        "runtime_registration":descriptor,"runtime_launch":descriptor,"environment_document":temp.path().join("environment.json")});
    assert!(provenance::specification(&config, &input).is_ok());
    for field in [
        "runtime_receipt",
        "target_plan",
        "runtime_registration",
        "runtime_launch",
    ] {
        let mut invalid = config.clone();
        invalid["bindings"]["provenance"][field]["sha256"] = json!("bad");
        assert!(provenance::specification(&invalid, &input).is_err());
    }
    config["bindings"]["source_fingerprints"]["runner_frontend"] =
        json!(format!("sha256:{}", "f".repeat(64)));
    assert!(provenance::specification(&config, &input).is_err());
}

#[test]
fn task_client_uses_managed_plaintext_options_without_password_argv_or_environment() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp
        .path()
        .join(if cfg!(windows) { "mysql.exe" } else { "mysql" });
    let header: &[u8] = if cfg!(windows) {
        b"MZ\0\0"
    } else if cfg!(target_os = "linux") {
        b"\x7fELF"
    } else {
        &[0xfe, 0xed, 0xfa, 0xcf]
    };
    fs::write(&executable, header).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let binding = json!({"control":{"host":"127.0.0.1","port":3307,"database":"ryframe_native_fixture","username":"fixture","password":"file-secret-quote\"-slash\\-line\n","tls_mode":"required"},
        "mysql_client":executable,"server_uuid":"baf03c60-671c-47f1-8504-d55640cc108e"});
    let (command, options) = mysql::command(&binding).unwrap();
    let arguments = command
        .get_args()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>();
    assert!(arguments[0].starts_with("--defaults-file="));
    assert!(arguments.iter().all(|value| !value.contains("file-secret")));
    assert!(command.get_envs().all(|(_, value)| {
        value.is_none_or(|value| !value.to_string_lossy().contains("file-secret"))
    }));
    let content = fs::read_to_string(options.path()).unwrap();
    assert!(content.contains("password=\"file-secret-quote\\\"-slash\\\\-line\\n\""));
    let path = options.path().to_owned();
    drop(options);
    assert!(!path.exists());
    let mut invalid = binding;
    invalid["control"]
        .as_object_mut()
        .unwrap()
        .remove("password");
    invalid["control"]["password_env"] = json!("OLD_SECRET");
    assert!(mysql::command(&invalid).is_err());
}

#[test]
fn job_artifact_binding_is_explicit_and_rejects_a_different_controller() {
    let temp = tempfile::tempdir().unwrap();
    let mut input = input(temp.path());
    let config = json!({"contract":{"workloads":{"api":[{"job":{"kind":"export"}}]}}});
    let mut invalid = input.native_tool.clone();
    invalid.sha256 = "0".repeat(64);
    input.native_tool = invalid;
    let error = jobs::ArtifactGuard::before(&config, &input, temp.path())
        .err()
        .unwrap();
    assert_eq!(error.category, "native_tool_binding");
    assert!(!temp.path().join("native-tool-before.json").exists());
    input.native_tool = super::super::super::job_evidence::artifact::current_binding().unwrap();
    let mut guard = jobs::ArtifactGuard::before(&config, &input, temp.path()).unwrap();
    guard.after().unwrap();
    let before: Value =
        serde_json::from_slice(&fs::read(temp.path().join("native-tool-before.json")).unwrap())
            .unwrap();
    let after: Value =
        serde_json::from_slice(&fs::read(temp.path().join("native-tool-after.json")).unwrap())
            .unwrap();
    assert_eq!(before, after);
    assert_eq!(before, json!(input.native_tool));
}
