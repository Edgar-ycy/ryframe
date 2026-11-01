//! 真实 Git 快照和普通文件验证秘密文件生产入口，不启动服务。
use super::super::{
    cli::FixtureEnvironmentCommand as Command,
    recovery::{
        clone::native::{json as document, source},
        fixture_control::environment::secrets,
    },
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command as Process,
};

const NAMES: [&str; 7] = [
    "mysql-client.cnf",
    "rustfs-access-key.txt",
    "rustfs-secret-key.txt",
    "redis-password.txt",
    "reset-admin-password.txt",
    "jwt-secret.txt",
    "metrics-token.txt",
];

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    target: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = super::super::workspace::root_dir().join(".local-tests/native-secret-files");
        fs::create_dir_all(&base).unwrap();
        let directory = tempfile::tempdir_in(base).unwrap();
        let root = directory.path().to_path_buf();
        fs::create_dir(root.join(".local-tests")).unwrap();
        let source = repository(&root, "原始 G");
        let target = repository(&root, "目标 G");
        let secrets = execution(&source).join(".local-tests/reference-fixture/secrets");
        fs::create_dir(&secrets).unwrap();
        for name in NAMES {
            fs::write(secrets.join(name), format!("冻结{name}\r\n")).unwrap();
        }
        Self {
            _directory: directory,
            root,
            source,
            target,
        }
    }
    fn rotated(&self) -> PathBuf {
        execution(&self.source).join(".local-tests/reference-fixture/新秘密集")
    }
    fn rotate(&self) -> Command {
        Command::RotateSecrets {
            fixture: self.source.clone(),
            output: self.rotated(),
        }
    }
    fn bootstrap(&self) -> Command {
        Command::BootstrapSecrets {
            source_fixture: self.source.clone(),
            source_secrets: self.rotated(),
            fixture: self.target.clone(),
        }
    }
}

fn assert_binding(value: &serde_json::Value, path: &Path) {
    let bytes = fs::read(path).unwrap();
    assert_eq!(Path::new(value["path"].as_str().unwrap()), path);
    assert_eq!(value["bytes"], json!(bytes.len()));
    assert_eq!(value["sha256"], hex::encode(Sha256::digest(&bytes)));
}

fn repository(root: &Path, name: &str) -> PathBuf {
    let backend = root.join(".local-tests").join(name);
    fs::create_dir_all(backend.join(".local-tests/reference-fixture")).unwrap();
    fs::write(backend.join(".gitignore"), "/.local-tests/\n").unwrap();
    fs::write(backend.join("input.txt"), "真实来源\n").unwrap();
    for args in [
        vec!["init"],
        vec!["config", "--local", "core.longpaths", "true"],
        vec!["add", "."],
        vec!["commit", "-m", "test(recovery): 初始化秘密文件夹具"],
    ] {
        let result = Process::new("git")
            .args([
                "-c",
                "user.name=夹具",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.autocrlf=false",
            ])
            .args([
                "-c",
                &format!("core.hooksPath={}", root.join("no-hooks").display()),
            ])
            .args(args)
            .current_dir(&backend)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let generated = source::protocol::request(&json!({"action":"snapshot","root":backend}))
        .unwrap()["snapshot"]
        .clone();
    let path = root.join(".local-tests").join(format!("{name}.json"));
    document::write_new(
        &path,
        &json!({"format_version":1,"fixture":"business","status":"ready",
        "sources":{"backend":generated},"paths":{"backend":backend,"frontend":backend},
        "generated":{"backend":generated}}),
    )
    .unwrap();
    path
}

fn execution(fixture: &Path) -> PathBuf {
    PathBuf::from(
        document::read(fixture).unwrap()["paths"]["backend"]
            .as_str()
            .unwrap(),
    )
}

#[test]
fn native_rotate_then_bootstrap_preserves_raw_files_and_receipt_bindings() {
    let fixture = Fixture::new();
    assert_eq!(
        secrets::run(&fixture.root, &fixture.rotate()).unwrap(),
        json!({"status":"generated","services_started":false,"remote_writes":0})
    );
    let rotated = document::read(&fixture.rotated().join("secret-set.json")).unwrap();
    let original = execution(&fixture.source).join(".local-tests/reference-fixture/secrets");
    for name in NAMES {
        assert_binding(&rotated["source"][name], &original.join(name));
        assert_binding(&rotated["files"][name], &fixture.rotated().join(name));
        if name != "reset-admin-password.txt" {
            assert!(
                fs::read(original.join(name)).unwrap()
                    == fs::read(fixture.rotated().join(name)).unwrap(),
                "非密码秘密文件必须逐字节保持：{name}"
            );
        }
    }
    let password = fs::read(fixture.rotated().join("reset-admin-password.txt")).unwrap();
    assert_eq!(password.len(), 48);
    assert!(&password[..4] == b"Aa1!", "密码前缀必须满足约定");
    assert!(password[47] == b'\n', "密码文件必须以换行结束");
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    assert!(
        URL_SAFE_NO_PAD
            .decode(&password[4..47])
            .is_ok_and(|bytes| bytes.len() == 32),
        "密码 token 必须是无填充 URL-safe base64 编码的 32 字节随机数"
    );
    assert!(secrets::run(&fixture.root, &fixture.rotate()).is_err());
    assert_eq!(
        secrets::run(&fixture.root, &fixture.bootstrap()).unwrap(),
        json!({"status":"imported","services_started":false,"remote_writes":0})
    );
    let target = execution(&fixture.target).join(".local-tests/reference-fixture/secrets");
    let receipt = document::read(&target.join("bootstrap.json")).unwrap();
    assert_binding(&receipt["source_fixture"], &fixture.source);
    assert_binding(&receipt["target_fixture"], &fixture.target);
    for name in NAMES {
        assert!(
            fs::read(target.join(name)).unwrap() == fs::read(fixture.rotated().join(name)).unwrap(),
            "导入秘密文件必须逐字节相同：{name}"
        );
        assert_binding(&receipt["target_files"][name], &target.join(name));
        assert_eq!(receipt["source_files"][name], rotated["files"][name]);
    }
    assert!(secrets::run(&fixture.root, &fixture.bootstrap()).is_err());
}

#[test]
fn native_secrets_reject_failed_sources_drift_and_wrong_directory_without_publishing() {
    let fixture = Fixture::new();
    let source = execution(&fixture.source).join(".local-tests/reference-fixture/secrets");
    fs::write(source.join("failed.json"), b"{}").unwrap();
    assert!(secrets::run(&fixture.root, &fixture.rotate()).is_err());
    assert!(!fixture.rotated().exists());
    fs::remove_file(source.join("failed.json")).unwrap();
    fs::write(execution(&fixture.source).join("input.txt"), "来源漂移").unwrap();
    assert!(secrets::run(&fixture.root, &fixture.rotate()).is_err());
    assert!(!fixture.rotated().exists());
    fs::write(execution(&fixture.source).join("input.txt"), "真实来源\n").unwrap();
    let wrong = Command::RotateSecrets {
        fixture: fixture.source.clone(),
        output: fixture.root.join(".local-tests/外部"),
    };
    assert!(secrets::run(&fixture.root, &wrong).is_err());
    let raw = fs::read(&fixture.source).unwrap();
    let mut changed = document::read(&fixture.source).unwrap();
    changed["generated"]["backend"]["head"] = json!("a".repeat(40));
    fs::write(&fixture.source, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(secrets::run(&fixture.root, &fixture.rotate()).is_err());
    assert!(!fixture.rotated().exists());
    fs::write(&fixture.source, raw).unwrap();
    let unrelated = fixture.root.join("无关源码");
    fs::create_dir(&unrelated).unwrap();
    let raw = fs::read(&fixture.source).unwrap();
    let mut external = document::read(&fixture.source).unwrap();
    external["paths"]["backend"] = json!(unrelated);
    fs::write(&fixture.source, serde_json::to_vec(&external).unwrap()).unwrap();
    let error = secrets::run(&fixture.root, &wrong).unwrap_err().to_string();
    assert!(error.contains("夹具执行目录无效"));
    assert!(error.contains("路径必须位于当前后端 .local-tests"));
    assert!(!fixture.root.join(".local-tests/外部").exists());
    assert!(fs::read_dir(&unrelated).unwrap().next().is_none());
    fs::write(&fixture.source, raw).unwrap();
    secrets::run(&fixture.root, &fixture.rotate()).unwrap();
    fs::write(fixture.rotated().join("failed.json"), b"{}").unwrap();
    assert!(secrets::run(&fixture.root, &fixture.bootstrap()).is_err());
    fs::remove_file(fixture.rotated().join("failed.json")).unwrap();
    let bound = fs::read(fixture.rotated().join("secret-set.json")).unwrap();
    fs::write(fixture.rotated().join("rustfs-access-key.txt"), b"changed").unwrap();
    assert!(secrets::run(&fixture.root, &fixture.bootstrap()).is_err());
    assert!(
        !execution(&fixture.target)
            .join(".local-tests/reference-fixture/secrets")
            .exists()
    );
    assert_eq!(
        fs::read(fixture.rotated().join("secret-set.json")).unwrap(),
        bound
    );
}

#[test]
fn native_bootstrap_detects_mid_copy_source_and_fixture_drift_without_success_receipt() {
    enum Drift {
        SourceSecret,
        SourceFixture,
        TargetFixture,
    }
    for drift in [
        Drift::SourceSecret,
        Drift::SourceFixture,
        Drift::TargetFixture,
    ] {
        let fixture = Fixture::new();
        secrets::run(&fixture.root, &fixture.rotate()).unwrap();
        let target = execution(&fixture.target).join(".local-tests/reference-fixture/secrets");
        assert!(!target.exists());
        let path = match drift {
            Drift::SourceSecret => fixture.rotated().join("rustfs-secret-key.txt"),
            Drift::SourceFixture => fixture.source.clone(),
            Drift::TargetFixture => fixture.target.clone(),
        };
        let before = fs::read(&path).unwrap();
        let mut changed = before.clone();
        changed.extend_from_slice(b" \n");
        let first_bytes = fs::read(fixture.rotated().join(NAMES[0])).unwrap();
        let mut synced = Vec::new();
        let error = secrets::run_with(&fixture.root, &fixture.bootstrap(), |written| {
            if synced.is_empty() {
                // 回调可达意味着同一生产前置核验已通过；首文件已完成实际写入与同步。
                assert_eq!(written, target.join(NAMES[0]));
                assert!(
                    fs::read(written).unwrap() == first_bytes,
                    "首个同步文件必须保持原字节"
                );
                assert!(!target.join(NAMES[1]).exists());
                assert!(!target.join("bootstrap.json").exists());
                assert!(
                    fs::read(&path).unwrap() == before,
                    "变异前输入必须保持原字节"
                );
                fs::write(&path, &changed).unwrap();
            }
            synced.push(written.to_path_buf());
            Ok(())
        })
        .unwrap_err()
        .to_string();
        assert_eq!(synced, NAMES.map(|name| target.join(name)));
        assert!(target.is_dir());
        assert!(!target.join("bootstrap.json").exists());
        assert_eq!(
            document::read(&target.join("failed.json")).unwrap(),
            json!({"status":"failed","remote_writes":0,"services_started":false})
        );
        assert!(fs::read(&path).unwrap() == changed, "核验不得恢复变异输入");
        for name in NAMES {
            let expected =
                if matches!(drift, Drift::SourceSecret) && name == "rustfs-secret-key.txt" {
                    before.clone()
                } else {
                    fs::read(fixture.rotated().join(name)).unwrap()
                };
            assert!(
                fs::read(target.join(name)).unwrap() == expected,
                "保留的目标文件必须保持复制前原字节：{name}"
            );
        }
        assert!(error.contains("本地目录已写入并保留"));
        assert!(error.contains("失败标记已同步"));
        let (stage, reason) = match drift {
            Drift::SourceSecret => ("复核秘密源", "秘密文件内容或绑定在核验期间变化"),
            Drift::SourceFixture | Drift::TargetFixture => {
                ("复核两侧夹具与源码快照", "业务夹具收据绑定发生变化")
            }
        };
        assert!(error.contains(&format!("阶段“{stage}”失败：{reason}")));
        for name in NAMES {
            let raw = fs::read(fixture.rotated().join(name)).unwrap();
            assert!(!error.contains(std::str::from_utf8(&raw).unwrap().trim()));
        }
    }
}

#[test]
fn native_bootstrap_partial_write_failures_preserve_inputs_and_redact_untrusted_errors() {
    enum Fault {
        Io,
        Unknown,
    }
    let secret = "Secret1!不可信错误消息不得公开";
    for fault in [Fault::Io, Fault::Unknown] {
        let fixture = Fixture::new();
        secrets::run(&fixture.root, &fixture.rotate()).unwrap();
        let target = execution(&fixture.target).join(".local-tests/reference-fixture/secrets");
        assert!(!target.exists());
        let inputs = NAMES
            .iter()
            .map(|name| fixture.rotated().join(name))
            .chain([
                fixture.source.clone(),
                fixture.target.clone(),
                fixture.rotated().join("secret-set.json"),
            ])
            .collect::<Vec<_>>();
        let before = inputs
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>();
        let mut calls = 0;
        let error = secrets::run_with(&fixture.root, &fixture.bootstrap(), |written| {
            calls += 1;
            assert_eq!(calls, 1);
            assert_eq!(written, target.join(NAMES[0]));
            assert!(
                fs::read(written).unwrap() == before[0],
                "首个同步文件必须保持原字节"
            );
            assert!(!target.join(NAMES[1]).exists());
            match fault {
                Fault::Io => {
                    Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, secret).into())
                }
                Fault::Unknown => Err(secret.into()),
            }
        })
        .unwrap_err()
        .to_string();
        assert_eq!(calls, 1);
        assert!(target.is_dir());
        assert_eq!(fs::read_dir(&target).unwrap().count(), 2);
        assert!(
            fs::read(target.join(NAMES[0])).unwrap() == before[0],
            "部分输出必须保留首文件原字节"
        );
        for name in &NAMES[1..] {
            assert!(!target.join(name).exists());
        }
        assert!(!target.join("bootstrap.json").exists());
        assert_eq!(
            document::read(&target.join("failed.json")).unwrap(),
            json!({"status":"failed","remote_writes":0,"services_started":false})
        );
        assert!(
            inputs
                .iter()
                .map(|path| fs::read(path).unwrap())
                .collect::<Vec<_>>()
                == before,
            "中途失败不得修改秘密源文件、manifest 或两侧夹具原字节"
        );
        assert!(error.contains("阶段“复制秘密文件”失败"));
        assert!(error.contains("本地目录已写入并保留"));
        assert!(error.contains("失败标记已同步"));
        assert!(error.contains(target.to_str().unwrap()));
        match fault {
            Fault::Io => {
                assert!(error.contains("kind=PermissionDenied"));
                assert!(error.contains("raw_os_error=None"));
            }
            Fault::Unknown => assert!(error.contains("上游核验未完成，细因待检查")),
        }
        assert!(!error.contains(secret));
        for raw in &before[..NAMES.len()] {
            assert!(!error.contains(std::str::from_utf8(raw).unwrap().trim()));
        }
    }
}
