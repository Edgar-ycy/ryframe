use super::*;

fn fixture() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/consumer-protocol-tests");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn arguments() -> Vec<String> {
    [
        "--mode",
        "formal",
        "--openapi",
        "openapi.json",
        "--backend-commit",
        "abc",
        "--backend-repository",
        "owner/repo",
        "--require-pin",
        "true",
    ]
    .map(str::to_owned)
    .to_vec()
}

#[test]
fn context_has_exact_seven_fields_and_ordered_configuration() {
    let root = fixture();
    std::fs::create_dir(root.path().join("config")).unwrap();
    std::fs::create_dir(root.path().join(".local-tests")).unwrap();
    std::fs::write(root.path().join(CONFIGURATION[0]), "[verification]\n").unwrap();
    std::fs::write(root.path().join(CONFIGURATION[1]), "[verification]\n").unwrap();
    let configuration = Configuration::capture(root.path()).unwrap();
    let path = root.path().join(".local-tests/frontend-origin-tool.json");
    let binding = Binding {
        path,
        bytes: 12,
        sha256: "a".repeat(64),
    };
    let bytes = encode(
        root.path(),
        root.path(),
        &root.path().join("venv/python"),
        &arguments(),
        &configuration.rows,
        binding,
    )
    .unwrap();
    let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let names = document
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        names,
        [
            "format_version",
            "arguments",
            "backend_root",
            "frontend_root",
            "python_program",
            "origin_tool_binding",
            "tool_configuration"
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(document["format_version"], 1);
    assert_eq!(document["arguments"], serde_json::json!(arguments()));
    assert_eq!(document["tool_configuration"][0]["path"], CONFIGURATION[0]);
    assert_eq!(document["tool_configuration"][1]["path"], CONFIGURATION[1]);
    assert_eq!(
        document["origin_tool_binding"].as_object().unwrap().len(),
        3
    );
    configuration.verify().unwrap();
}

#[test]
fn configuration_detects_missing_file_becoming_present() {
    let root = fixture();
    std::fs::create_dir(root.path().join("config")).unwrap();
    std::fs::create_dir(root.path().join(".local-tests")).unwrap();
    std::fs::write(root.path().join(CONFIGURATION[0]), "").unwrap();
    let configuration = Configuration::capture(root.path()).unwrap();
    std::fs::write(root.path().join(CONFIGURATION[1]), "").unwrap();
    assert!(configuration.verify().is_err());
}

#[test]
fn malformed_arguments_leave_failure_evidence_without_context() {
    let root = fixture();
    assert!(prepare(root.path(), root.path(), Vec::new()).is_err());
    let directory = root.path().join(".local-tests/consumer-contexts");
    let evidence = std::fs::read_dir(directory)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(evidence.join("failure.json").is_file());
    assert!(!evidence.join("context.json").exists());
    assert!(validate_arguments(&arguments()).is_ok());
    let mut extra = arguments();
    extra.push("secret".into());
    assert!(validate_arguments(&extra).is_err());
}

#[test]
fn all_context_paths_must_be_absolute() {
    let root = fixture();
    let binding = Binding {
        path: "relative.json".into(),
        bytes: 1,
        sha256: "a".repeat(64),
    };
    assert!(
        encode(
            root.path(),
            root.path(),
            Path::new("python"),
            &arguments(),
            &[],
            binding
        )
        .is_err()
    );
}

#[test]
fn python_origin_run_needs_only_backend_without_frontend_or_fixed_binding() {
    let root = fixture();
    let backend = root.path().join("后端目录 with spaces");
    std::fs::create_dir(&backend).unwrap();
    std::fs::create_dir(backend.join("config")).unwrap();
    std::fs::write(
        backend.join("config/tools.toml"),
        "frontend_dir = '../missing-前端目录'\n",
    )
    .unwrap();
    let binding = backend.join(".local-tests/frontend-origin-tool.json");
    assert!(!binding.exists());
    assert!(!root.path().join("missing-前端目录").exists());
    let (first_root, first) = python_origin_run(&backend).unwrap();
    let (second_root, second) = python_origin_run(&backend).unwrap();
    assert_eq!(first_root, files::absolute_directory(&backend).unwrap());
    assert_eq!(second_root, first_root);
    assert!(first.starts_with(first_root.join(".local-tests/python-origin-runs")));
    assert!(first.is_dir() && second.is_dir());
    assert_ne!(first, second);
    assert!(uuid::Uuid::parse_str(first.file_name().unwrap().to_str().unwrap()).is_ok());
    assert!(!binding.exists());
    assert!(!root.path().join("missing-前端目录").exists());
    assert!(!backend.join(".local-tests/consumer-contexts").exists());
    assert!(!first.join("context.json").exists());
    assert_eq!(std::fs::read_dir(first).unwrap().count(), 0);
}

#[test]
fn python_origin_preparation_rejects_invalid_backend_before_program_execution() {
    let root = fixture();
    let regular_file = root.path().join("file");
    std::fs::write(&regular_file, "ordinary file").unwrap();
    let prepare: fn(&Path) -> Result<OriginBinding> = prepare_python_origin;
    for path in [
        root.path().join(".."),
        root.path().join("SECRET_SENTINEL\n"),
        root.path().join("missing"),
        regular_file,
    ] {
        let error = prepare(&path).err().unwrap().to_string();
        assert!(!error.contains("SECRET_SENTINEL"));
    }
    assert!(!root.path().join(".local-tests").exists());
}
