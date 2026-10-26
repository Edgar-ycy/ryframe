//! 显式共享 target 的真实 Cargo 回归；由资源控制器独占运行。
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

struct Build {
    outputs: BTreeMap<PathBuf, Generated>,
    fingerprint: Vec<u8>,
    fresh: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct Generated {
    fingerprint: Vec<u8>,
    modified: SystemTime,
}

#[test]
#[ignore = "需要独占 CARGO_TARGET_DIR；会执行真实 Cargo A→B→A"]
fn shared_target_rechecks_macro_sources_across_roots() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = source.parent().unwrap().parent().unwrap();
    let target =
        PathBuf::from(std::env::var_os("CARGO_TARGET_DIR").expect("必须显式选择共享 target"));
    assert!(target.is_absolute());
    let owned = workspace.join(".local-tests");
    fs::create_dir_all(&owned).unwrap();
    let fixture = tempfile::Builder::new()
        .prefix("macro-source-cache-")
        .tempdir_in(owned)
        .unwrap();
    // 失败保留完整源码、Cargo JSON 和 stderr，不能以重试或清理掩盖旧产物。
    let root = fixture.keep();
    let a = prepare(source, &root.join("a"));
    let b = prepare(source, &root.join("b"));
    resolve_lock(&a, &root, "a");
    resolve_lock(&b, &root, "b");
    let first = build(&a, &target, &root, "a-test", "test");
    let same = build(&a, &target, &root, "a-test-repeat", "test");
    assert_eq!(first.fingerprint, same.fingerprint);
    assert_eq!(first.outputs, same.outputs);
    let checked = build(&a, &target, &root, "a-check", "check");
    assert_eq!(first.fingerprint, checked.fingerprint);
    let identical = build(&b, &target, &root, "b-identical", "test");
    assert_eq!(first.fingerprint, identical.fingerprint);
    fs::write(b.join("src/cache_probe.rs"), "// 新增合法 Rust 输入\n").unwrap();
    let changed = build(&b, &target, &root, "b-added", "build");
    assert_ne!(first.fingerprint, changed.fingerprint);
    assert!(!changed.fresh, "变更必须实际重编译宏");
    fs::write(b.join("src/cache_probe.rs"), "// 改写合法 Rust 输入\n").unwrap();
    let edited = build(&b, &target, &root, "b-edited", "check");
    assert_ne!(changed.fingerprint, edited.fingerprint);
    assert!(!edited.fresh);
    fs::remove_file(b.join("src/cache_probe.rs")).unwrap();
    let deleted = build(&b, &target, &root, "b-deleted", "build");
    assert_eq!(first.fingerprint, deleted.fingerprint);
    assert!(!deleted.fresh);
    write_probe(source, &b, "B");
    let semantic = build(&b, &target, &root, "b-semantic", "test");
    assert_ne!(first.fingerprint, semantic.fingerprint);
    assert!(!semantic.fresh);
    let returned = build(&a, &target, &root, "a-return", "test");
    assert_eq!(first.fingerprint, returned.fingerprint);
    assert!(!returned.fresh, "返回 A 不能复用 B 的宏");
    let stable = build(&a, &target, &root, "a-stable", "test");
    assert_eq!(returned.fingerprint, stable.fingerprint);
    assert_eq!(returned.outputs, stable.outputs);
    verify_sentinel(&a, &target, &root, &stable.outputs);
    println!("完整 Cargo 证据保留于 {}", root.display());
}

fn verify_sentinel(
    package: &Path,
    target: &Path,
    evidence: &Path,
    outputs: &BTreeMap<PathBuf, Generated>,
) {
    let markers: BTreeMap<_, _> = outputs
        .keys()
        .map(|output| {
            let path = output.join("macro-source-always-check");
            let marker = format!(
                "owned regression sentinel for {} at {}",
                evidence.display(),
                path.display()
            );
            (path, marker)
        })
        .collect();
    // 全部路径先核实不存在；任何已有对象都不得被覆盖或清理。
    for path in markers.keys() {
        assert_eq!(
            fs::symlink_metadata(path).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
    }
    for (path, marker) in &markers {
        let mut owned = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        owned.write_all(marker.as_bytes()).unwrap();
        owned.sync_all().unwrap();
    }
    let result = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(package)
        .args(["test", "--offline", "--locked", "--target-dir"])
        .arg(target)
        .args(["--message-format=json", "--test", "patch_route"])
        .output()
        .unwrap();
    fs::write(evidence.join("sentinel.stderr"), &result.stderr).unwrap();
    fs::write(evidence.join("sentinel.stdout"), &result.stdout).unwrap();
    for (path, marker) in &markers {
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_file());
        assert_eq!(fs::read(path).unwrap(), marker.as_bytes());
    }
    // 全部内容核对后只清理本用例的 marker；即使命令错误成功也不遗留已确认对象。
    for path in markers.keys() {
        fs::remove_file(path).unwrap();
    }
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("必须不存在"));
}

fn prepare(source: &Path, root: &Path) -> PathBuf {
    let package = root.join("macro");
    fs::create_dir_all(&package).unwrap();
    copy_tree(&source.join("src"), &package.join("src"));
    copy_tree(&source.join("tests"), &package.join("tests"));
    for name in ["Cargo.toml", "build.rs"] {
        fs::copy(source.join(name), package.join(name)).unwrap();
    }
    fs::copy(
        source
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("Cargo.lock"),
        root.join("Cargo.lock"),
    )
    .unwrap();
    let workspace = source.parent().unwrap().parent().unwrap();
    fs::write(
        root.join("Cargo.toml"),
        workspace_manifest(workspace, source),
    )
    .unwrap();
    write_probe(source, &package, "A");
    package
}

fn workspace_manifest(root: &Path, source: &Path) -> String {
    let original: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("Cargo.toml")).unwrap()).unwrap();
    let package: toml::Value =
        toml::from_str(&fs::read_to_string(source.join("Cargo.toml")).unwrap()).unwrap();
    let mut dependencies = toml::map::Map::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        for (name, value) in package[section].as_table().unwrap() {
            if value.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                dependencies.insert(
                    name.clone(),
                    original["workspace"]["dependencies"][name.as_str()].clone(),
                );
            }
        }
    }
    let mut workspace = toml::map::Map::new();
    for field in ["package", "lints", "resolver"] {
        if let Some(value) = original["workspace"].get(field) {
            workspace.insert(field.into(), value.clone());
        }
    }
    workspace.insert("members".into(), toml::Value::Array(vec!["macro".into()]));
    workspace.insert("dependencies".into(), toml::Value::Table(dependencies));
    let mut manifest = toml::map::Map::new();
    manifest.insert("workspace".into(), toml::Value::Table(workspace));
    for field in ["patch", "profile"] {
        if let Some(value) = original.get(field) {
            manifest.insert(field.into(), value.clone());
        }
    }
    let mut manifest = toml::Value::Table(manifest);
    absolute_paths(&mut manifest, root);
    toml::to_string(&manifest).unwrap()
}

fn absolute_paths(value: &mut toml::Value, root: &Path) {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                if ["path", "readme", "license-file"].contains(&key.as_str()) {
                    if let Some(path) = value.as_str() {
                        *value = root
                            .join(path)
                            .canonicalize()
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .into();
                    }
                } else {
                    absolute_paths(value, root);
                }
            }
        }
        toml::Value::Array(values) => {
            for value in values {
                absolute_paths(value, root);
            }
        }
        _ => {}
    }
}

fn write_probe(source: &Path, package: &Path, expected: &str) {
    let original = fs::read_to_string(source.join("src/lib.rs")).unwrap();
    let probe = r#"
#[proc_macro]
pub fn source_cache_probe(_: TokenStream) -> TokenStream {
    format!("{:?}", SOURCE_CACHE_LITERAL).parse().expect("字面量必须可解析")
}
"#;
    fs::write(
        package.join("src/lib.rs"),
        original + &probe.replace("SOURCE_CACHE_LITERAL", &format!("{expected:?}")),
    )
    .unwrap();
    let consumer = fs::read_to_string(source.join("tests/patch_route.rs")).unwrap();
    fs::write(package.join("tests/patch_route.rs"), format!("{consumer}\n#[test]\nfn macro_dll_emits_current_source_literal() {{\n    assert_eq!(ryframe_macro::source_cache_probe!(), {expected:?});\n}}\n")).unwrap();
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        assert!(!entry.file_type().unwrap().is_symlink());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &destination.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
        }
    }
}

fn build(package: &Path, target: &Path, evidence: &Path, label: &str, action: &str) -> Build {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .current_dir(package)
        .args([action, "--offline", "--locked", "--target-dir"])
        .arg(target)
        .args(["--message-format=json"]);
    if action == "test" {
        command.args(["--test", "patch_route"]);
    } else {
        command.arg("--lib");
    }
    let started = std::time::Instant::now();
    let output = command.output().unwrap();
    fs::write(evidence.join(format!("{label}.jsonl")), &output.stdout).unwrap();
    fs::write(evidence.join(format!("{label}.stderr")), &output.stderr).unwrap();
    fs::write(
        evidence.join(format!("{label}.elapsed")),
        format!("{:?}", started.elapsed()),
    )
    .unwrap();
    assert!(
        output.status.success(),
        "{label} 失败，保留 {}",
        evidence.display()
    );
    let messages: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    collect_outputs(&messages)
}

fn collect_outputs(messages: &[Value]) -> Build {
    let artifacts: Vec<_> = messages
        .iter()
        .filter(|value| {
            value["reason"] == "compiler-artifact"
                && value["target"]["name"] == "ryframe_macro"
                && value["target"]["kind"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|kind| kind == "proc-macro")
        })
        .collect();
    let package_id = artifacts.first().expect("缺少精确宏产物")["package_id"]
        .as_str()
        .unwrap();
    assert!(
        artifacts
            .iter()
            .all(|artifact| artifact["package_id"].as_str() == Some(package_id))
    );
    let mut outputs: BTreeMap<PathBuf, Generated> = BTreeMap::new();
    for script in messages.iter().filter(|value| {
        value["reason"] == "build-script-executed"
            && value["package_id"].as_str() == Some(package_id)
    }) {
        let output = PathBuf::from(script["out_dir"].as_str().unwrap());
        assert!(output.is_absolute());
        let path = output.join("macro-source.sha256");
        let generated = Generated {
            fingerprint: fs::read(&path).unwrap(),
            modified: fs::metadata(path).unwrap().modified().unwrap(),
        };
        if let Some(previous) = outputs.get(&output) {
            assert_eq!(previous, &generated, "重复 out_dir 不能具有不同内容或时间");
        }
        outputs.insert(output, generated);
    }
    let fingerprint = outputs
        .values()
        .next()
        .expect("缺少当前宏构建脚本输出")
        .fingerprint
        .clone();
    assert!(
        outputs
            .values()
            .all(|generated| generated.fingerprint == fingerprint),
        "同一宏包全部输出必须绑定相同源码指纹"
    );
    Build {
        outputs,
        fingerprint,
        fresh: artifacts
            .iter()
            .all(|artifact| artifact["fresh"].as_bool().unwrap()),
    }
}

fn resolve_lock(package: &Path, evidence: &Path, label: &str) {
    // 仅离线解析和裁剪种子锁；核对通过后所有编译均锁定，禁止升级或网络回退。
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(package)
        .args(["metadata", "--offline", "--format-version", "1"])
        .output()
        .unwrap();
    fs::write(evidence.join(format!("{label}-lock.json")), &output.stdout).unwrap();
    fs::write(
        evidence.join(format!("{label}-lock.stderr")),
        &output.stderr,
    )
    .unwrap();
    assert!(
        output.status.success(),
        "离线锁解析失败，保留 {}",
        evidence.display()
    );
    verify_lock(package.parent().unwrap());
}

fn verify_lock(root: &Path) {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let seed: toml::Value =
        toml::from_str(&fs::read_to_string(workspace.join("Cargo.lock")).unwrap()).unwrap();
    let resolved: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("Cargo.lock")).unwrap()).unwrap();
    for package in resolved["package"].as_array().unwrap() {
        assert!(
            seed["package"].as_array().unwrap().iter().any(|original| {
                ["name", "version", "source", "checksum"]
                    .iter()
                    .all(|field| original.get(*field) == package.get(*field))
            }),
            "离线夹具解析不得引入种子锁以外的包版本或来源：{}",
            package["name"]
        );
    }
}
