use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{assets::MARKER, git, publish, source, verify};

#[path = "execution_window.rs"]
mod window_tests;

pub(crate) struct Fixture {
    _directory: tempfile::TempDir,
    pub(crate) backend: PathBuf,
    pub(crate) frontend: PathBuf,
    pub(crate) receipt: PathBuf,
    pub(crate) output: PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let base = crate::workspace::root_dir().join(".local-tests");
        Self::new_in(&base)
    }

    pub(crate) fn new_in(base: &Path) -> Self {
        fs::create_dir_all(base).unwrap();
        let directory = tempfile::tempdir_in(base).unwrap();
        let backend = directory.path().join("backend");
        let frontend = directory.path().join("frontend");
        for root in [&backend, &frontend] {
            initialize(root, &backend);
        }
        let sources = json!({"backend": snapshot(&backend), "frontend": snapshot(&frontend)});
        fs::write(backend.join("src/lib.rs"), "// 业务生成后的跟踪文件\n").unwrap();
        let model = backend.join("crates/order-business/src/resources/mod.rs");
        fs::create_dir_all(model.parent().unwrap()).unwrap();
        fs::write(&model, "// Order 完整模型夹具\n").unwrap();
        fs::write(frontend.join("src/business.txt"), "生成的前端业务资产\n").unwrap();
        fs::create_dir_all(backend.join(".local-tests")).unwrap();
        fs::write(
            backend.join(".local-tests/private.toml"),
            "private fixture\n",
        )
        .unwrap();
        let generated = json!({"backend": snapshot(&backend), "frontend": snapshot(&frontend)});
        let receipt = directory.path().join("fixture.json");
        let document = json!({"format_version":1,"fixture":"business","status":"ready",
            "fixture_sha256":crate::devex::runtime_native::artifacts::sha(&fs::read(model).unwrap()),
            "sources":sources,"paths":{"backend":backend,"frontend":frontend},"generated":generated});
        fs::write(&receipt, serde_json::to_vec(&document).unwrap()).unwrap();
        let output = backend.join(".local-tests/published");
        Self {
            _directory: directory,
            backend,
            frontend,
            receipt,
            output,
        }
    }

    /// 仅 Prepare 用例需要可发布的基础配置；其他 Fixture 初始化保持不变。
    pub(crate) fn prepare_environment_configuration(&self) {
        use crate::local_test_path::{LocalTestPathKind, validate_local_test_path};
        let clean = self.backend.join(".local-tests/prepare-clean-baseline");
        validate_local_test_path(&clean, &self.backend, LocalTestPathKind::NewDirectory).unwrap();
        fs::write(self.backend.join("config/app.toml"), b"[app]\n").unwrap();
        git::run(&self.backend, &["add", "--", "config/app.toml"]).unwrap();
        git::run(
            &self.backend,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@ryframe.invalid",
                "commit",
                "--quiet",
                "--only",
                "-m",
                "test(recovery): 夹具基础配置",
                "--",
                "config/app.toml",
            ],
        )
        .unwrap();
        git::run(
            &self.backend,
            &[
                "clone",
                "--no-hardlinks",
                "--",
                self.backend.to_str().unwrap(),
                clean.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert!(source::capture(&clean).unwrap().source.snapshot.clean);
        let original = snapshot(&clean);
        let generated = snapshot(&self.backend);
        let mut receipt: Value = serde_json::from_slice(&fs::read(&self.receipt).unwrap()).unwrap();
        receipt["sources"]["backend"] = original;
        receipt["generated"]["backend"] = generated;
        fs::write(&self.receipt, serde_json::to_vec(&receipt).unwrap()).unwrap();
    }
    fn verify(&self) -> crate::Result<Value> {
        let descriptor = crate::recovery::fixture_control::file_binding(
            &self.output.join("execution-source.json"),
            "执行源码清单",
        )?;
        verify(&self.backend, &descriptor)
    }

    pub(crate) fn publication(&self) -> Value {
        publish(&self.backend, &self.output).unwrap()
    }

    pub(crate) fn binding(&self) -> Value {
        crate::recovery::fixture_control::file_binding(
            &self
                .output
                .join("execution-source.json")
                .canonicalize()
                .unwrap(),
            "执行源码",
        )
        .unwrap()
    }

    fn marker(&self) -> Vec<u8> {
        let bytes = canonical(
            &json!({"candidate_sha256":crate::contract::sha256_hex(&fs::read(self.frontend.join("openapi/openapi.json")).unwrap()),
            "formal_source_sha256":crate::contract::sha256_hex(&fs::read(self.frontend.join("openapi/source.json")).unwrap()),
            "mode":"candidate","openapi_version":"3.1.0","schema_version":1}),
        );
        fs::write(self.frontend.join(MARKER), &bytes).unwrap();
        bytes
    }

    fn candidate(&self) -> Vec<u8> {
        let path = self.backend.join("openapi/openapi.json");
        let mut document: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        document["info"]["version"] = json!("0.0.2");
        let bytes = canonical(&document);
        fs::write(path, &bytes).unwrap();
        fs::write(self.frontend.join("openapi/openapi.json"), &bytes).unwrap();
        let mut receipt: Value = serde_json::from_slice(&fs::read(&self.receipt).unwrap()).unwrap();
        receipt["generated"] =
            json!({"backend":snapshot(&self.backend),"frontend":snapshot(&self.frontend)});
        fs::write(&self.receipt, serde_json::to_vec(&receipt).unwrap()).unwrap();
        self.marker()
    }

    fn reject_original_marker_drift(&self, bytes: &[u8]) {
        let marker: Value = serde_json::from_slice(bytes).unwrap();
        let mut changed = marker.clone();
        changed["candidate_sha256"] = json!("0".repeat(64));
        let compact = serde_json::to_vec(&marker).unwrap();
        let mut extra = marker;
        extra["unexpected"] = json!(true);
        let path = self.frontend.join(MARKER);
        for invalid in [
            None,
            Some(canonical(&changed)),
            Some(canonical(&extra)),
            Some(compact),
        ] {
            if let Some(invalid) = invalid {
                fs::write(&path, invalid).unwrap();
            } else {
                fs::remove_file(&path).unwrap();
            }
            assert!(self.verify().is_err());
            fs::write(&path, bytes).unwrap();
            self.verify().unwrap();
        }
    }
}

fn canonical(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    bytes
}

fn initialize(root: &Path, backend: &Path) {
    for name in ["src", "config", "openapi"] {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    fs::write(
        root.join("config/template.toml"),
        "source_template = true\n",
    )
    .unwrap();
    fs::write(
        root.join(".gitignore"),
        ".local-tests/\ntarget/\nnode_modules/\ndist/\n*.ignored\nopenapi/candidate.json\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "// 原始干净源码\n").unwrap();
    if root == backend {
        fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(root.join("README.md"), "fixture\n").unwrap();
        fs::create_dir_all(root.join("crates/ryframe")).unwrap();
        fs::write(root.join("crates/ryframe/Cargo.toml"), "fixture\n").unwrap();
        let model = root.join("tools/python/fixtures/order-business/src/resources/mod.rs");
        fs::create_dir_all(model.parent().unwrap()).unwrap();
        fs::write(model, "// Order 完整模型夹具\n").unwrap();
    }
    let bytes = canonical(
        &json!({"info":{"title":"RyFrame API","version":"0.0.1"},"openapi":"3.1.0","paths":{}}),
    );
    fs::write(root.join("openapi/openapi.json"), &bytes).unwrap();
    if root != backend {
        let metadata = json!({"schema_version":1,"backend_repository":"fixture/backend",
            "backend_commit":git::text(backend,&["rev-parse","HEAD"]).unwrap(),
            "openapi_path":"openapi/openapi.json","openapi_version":"3.1.0","sha256":crate::contract::sha256_hex(&bytes)});
        fs::write(root.join("openapi/source.json"), canonical(&metadata)).unwrap();
    }
    git::run(root, &["init", "--quiet", "--template="]).unwrap();
    git::run(root, &["add", "."]).unwrap();
    git::run(
        root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@ryframe.invalid",
            "commit",
            "--quiet",
            "-m",
            "test(source): 初始化原始源码",
        ],
    )
    .unwrap();
}

fn snapshot(root: &Path) -> Value {
    let value = source::capture(root).unwrap().source.snapshot;
    json!({"head":value.head,"patch_sha256":value.patch_sha256,
        "files":value.files.iter().map(|v| json!({"path":v.path,"sha256":v.sha256})).collect::<Vec<_>>()})
}

#[test]
fn execution_publication_preserves_original_and_rejects_changed_published_source() {
    let fixture = Fixture::new();
    let marker = fixture.candidate();
    let originals = [
        source::capture(&fixture.backend).unwrap(),
        source::capture(&fixture.frontend).unwrap(),
    ];
    let receipt = fs::read(&fixture.receipt).unwrap();
    let published = publish(&fixture.backend, &fixture.output).unwrap();
    assert_eq!(fixture.verify().unwrap(), published);
    assert_eq!(
        published["original"]["contract_assets"]["sha256"],
        crate::contract::sha256_hex(&marker)
    );
    assert_eq!(
        published["original"]["current"]["frontend"],
        source::inventory_value(&originals[1])
    );
    for (index, name) in ["backend", "frontend"].into_iter().enumerate() {
        let root = fixture.output.join(name);
        let actual = source::capture(&root).unwrap();
        assert!(actual.source.snapshot.clean);
        let mut expected = originals[index].files.clone();
        if name == "frontend" {
            assert!(!expected.iter().any(|file| file.path == MARKER));
            expected.push(source::FileRecord {
                path: MARKER.into(),
                sha256: crate::contract::sha256_hex(&marker),
            });
            expected.sort_by(|a, b| a.path.cmp(&b.path));
            git::run(&root, &["ls-files", "--error-unmatch", "--", MARKER]).unwrap();
            assert_eq!(fs::read(root.join(MARKER)).unwrap(), marker);
        }
        assert_eq!(actual.files, expected);
        assert_ne!(
            actual.source.snapshot.head,
            originals[index].source.snapshot.head
        );
        assert!(!root.join(".local-tests").exists());
        assert!(fs::symlink_metadata(root.join(".git")).unwrap().is_dir());
    }
    assert_eq!(source::capture(&fixture.backend).unwrap(), originals[0]);
    assert_eq!(source::capture(&fixture.frontend).unwrap(), originals[1]);
    assert_eq!(fs::read(&fixture.receipt).unwrap(), receipt);
    assert_eq!(fs::read(fixture.frontend.join(MARKER)).unwrap(), marker);
    fixture.reject_original_marker_drift(&marker);
    let published_marker = fixture.output.join("frontend").join(MARKER);
    fs::write(&published_marker, "{}\n").unwrap();
    assert!(fixture.verify().is_err());
    fs::write(published_marker, &marker).unwrap();
    fixture.verify().unwrap();
    fs::write(
        fixture.output.join("backend/src/lib.rs"),
        "// 发布根被篡改\n",
    )
    .unwrap();
    assert!(fixture.verify().is_err());
}

#[test]
fn execution_publication_rejects_existing_output_and_original_drift() {
    let fixture = Fixture::new();
    let published = publish(&fixture.backend, &fixture.output).unwrap();
    assert!(published["original"]["contract_assets"].is_null());
    let path = fixture.output.join("execution-source.json");
    let receipt = fs::read(&path).unwrap();
    assert!(publish(&fixture.backend, &fixture.output).is_err());
    assert_eq!(fs::read(&path).unwrap(), receipt);
    let original = source::capture(&fixture.frontend).unwrap();
    fixture.marker();
    assert_eq!(source::capture(&fixture.frontend).unwrap(), original);
    assert!(fixture.verify().is_err());
    fs::remove_file(fixture.frontend.join(MARKER)).unwrap();
    fixture.verify().unwrap();
    fs::write(
        fixture.frontend.join("src/business.txt"),
        "漂移的生成资产\n",
    )
    .unwrap();
    assert!(fixture.verify().is_err());
    let next = fixture.backend.join(".local-tests/another-output");
    assert!(publish(&fixture.backend, &next).is_err());
    assert!(!next.exists());
    fs::write(
        fixture.frontend.join("src/business.txt"),
        "生成的前端业务资产\n",
    )
    .unwrap();
    fs::write(
        fixture.backend.join("config/private.toml"),
        "private_fixture = true\n",
    )
    .unwrap();
    let mut document: Value = serde_json::from_slice(&fs::read(&fixture.receipt).unwrap()).unwrap();
    document["generated"]["backend"] = snapshot(&fixture.backend);
    fs::write(&fixture.receipt, serde_json::to_vec(&document).unwrap()).unwrap();
    let error = publish(&fixture.backend, &next).unwrap_err();
    assert!(error.to_string().contains("运行配置"));
    assert!(!next.exists());
}

#[test]
fn execution_publication_separates_managed_assets_from_unchanged_sources() {
    let fixture = Fixture::new();
    let document = publish(&fixture.backend, &fixture.output).unwrap();
    let root = fixture.output.join("backend");
    let frontend = fixture.output.join("frontend");
    // 这些只是隔离的数据样本，来源核验成功不代表它们具备构建或运行资格。
    for directory in [
        root.join(".local-tests"),
        frontend.join(".local-tests"),
        frontend.join("node_modules"),
        frontend.join("dist"),
    ] {
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("managed-fixture.txt"), "受管职责样本\n").unwrap();
    }
    assert!(source::capture(&root).unwrap().source.snapshot.clean);
    assert_eq!(fixture.verify().unwrap(), document);
    let unknown = root.join("extra.ignored");
    fs::write(&unknown, "未受管 ignored 文件\n").unwrap();
    assert!(fixture.verify().is_err());
    fs::remove_file(unknown).unwrap();
    for directory in [root.join("target"), root.join("src/.local-tests")] {
        fs::create_dir(&directory).unwrap();
        let file = directory.join("unmanaged.txt");
        fs::write(&file, "不允许根 target 或同名深层目录例外\n").unwrap();
        assert!(fixture.verify().is_err());
        fs::remove_file(file).unwrap();
        fs::remove_dir(directory).unwrap();
    }
    let managed = frontend.join(".local-tests");
    let saved = fixture._directory.path().join("saved-assets");
    fs::rename(&managed, &saved).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&saved, &managed).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&saved, &managed).unwrap();
    let observed = fixture.verify();
    #[cfg(windows)]
    fs::remove_dir(&managed).unwrap();
    #[cfg(unix)]
    fs::remove_file(&managed).unwrap();
    fs::rename(saved, managed).unwrap();
    assert!(observed.is_err());
    assert_eq!(fixture.verify().unwrap(), document);
    fs::write(
        root.join("config/template.toml"),
        "source_template = false\n",
    )
    .unwrap();
    assert!(fixture.verify().is_err());
}
