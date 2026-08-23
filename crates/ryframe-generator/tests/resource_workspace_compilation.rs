use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use ryframe_generator::{ResourceWorkspace, load_resource, render_resources, write_resource};

#[test]
#[ignore = "完整门禁在临时真实 Workspace 中运行 Cargo 与 vue-tsc"]
fn device_slice_compiles_in_temporary_real_workspaces() {
    let backend_source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("生成器应位于后端 Workspace/crates")
        .to_path_buf();
    let frontend_source = backend_source
        .parent()
        .expect("后端应有工作区父目录")
        .join("ryframe-vue3");
    assert!(frontend_source.join("node_modules").is_dir());

    let backend_parent = backend_source.join(".local-tests");
    let frontend_parent = frontend_source.join(".local-tests");
    fs::create_dir_all(&backend_parent).expect("应创建后端临时测试目录");
    fs::create_dir_all(&frontend_parent).expect("应创建前端临时测试目录");
    let backend = tempfile::Builder::new()
        .prefix("resource-workspace-")
        .tempdir_in(&backend_parent)
        .expect("应创建后端临时 Workspace");
    let frontend = tempfile::Builder::new()
        .prefix("resource-workspace-")
        .tempdir_in(&frontend_parent)
        .expect("应创建前端临时 Workspace");

    for file in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "rustfmt.toml",
    ] {
        fs::copy(backend_source.join(file), backend.path().join(file))
            .unwrap_or_else(|error| panic!("复制 {file} 失败：{error}"));
    }
    for directory in [".cargo", "catalog", "crates", "vendor", "xtask"] {
        copy_directory(
            &backend_source.join(directory),
            &backend.path().join(directory),
        );
    }
    prepare_frontend_workspace(&frontend_source, frontend.path());

    let device =
        load_resource(backend_source.join("crates/ryframe-generator/tests/fixtures/device.toml"))
            .expect("Device 清单应有效");
    let post = load_resource(backend_source.join("catalog/resources/post.toml"))
        .expect("临时 Workspace 中既有的 Post 清单应有效");
    let catalog = render_resources(&[device, post]).expect("Device 与既有资源应能共同生成");
    let first = write_resource(
        &catalog,
        "device",
        ResourceWorkspace {
            backend_root: backend.path(),
            frontend_root: Some(frontend.path()),
        },
    )
    .expect("Device 应写入临时 Workspace");
    assert!(!first.written.is_empty());
    let second = write_resource(
        &catalog,
        "device",
        ResourceWorkspace {
            backend_root: backend.path(),
            frontend_root: Some(frontend.path()),
        },
    )
    .expect("连续生成应成功");
    assert!(second.written.is_empty());
    assert!(second.removed.is_empty());
    for crate_name in [
        "ryframe-application",
        "ryframe-db",
        "ryframe-tenant-db",
        "ryframe-api",
    ] {
        let lib = backend
            .path()
            .join("crates")
            .join(crate_name)
            .join("src/lib.rs");
        let mut source = fs::read_to_string(&lib).expect("应读取 crate lib.rs");
        if !source
            .lines()
            .any(|line| line.trim() == "pub mod generated;")
        {
            source.push_str("\npub mod generated;\n");
        }
        fs::write(&lib, source).expect("应在临时副本接入 generated module");
    }
    register_device_frontend_contract(frontend.path());
    write_device_fake_transaction_test(backend.path());

    let cargo_fmt = Command::new("cargo")
        .args(["fmt", "--all", "--", "--check"])
        .current_dir(backend.path())
        .output()
        .expect("应检查生成 Rust 资产格式");
    assert_command_succeeded("cargo fmt --check", &cargo_fmt);

    let cargo = Command::new("cargo")
        .args([
            "check",
            "-p",
            "ryframe-application",
            "-p",
            "ryframe-db",
            "-p",
            "ryframe-tenant-db",
            "-p",
            "ryframe-api",
        ])
        .current_dir(backend.path())
        .env(
            "CARGO_TARGET_DIR",
            backend_source.join("target/resource-generator-workspace-check"),
        )
        .output()
        .expect("应运行临时后端 cargo check");
    assert_command_succeeded("cargo check", &cargo);

    let fake_test = Command::new("cargo")
        .args([
            "test",
            "-p",
            "ryframe-application",
            "--test",
            "generated_device_fake",
        ])
        .current_dir(backend.path())
        .env(
            "CARGO_TARGET_DIR",
            backend_source.join("target/resource-generator-workspace-check"),
        )
        .output()
        .expect("应运行生成 Fake 事务语义测试");
    assert_command_succeeded("generated Device fake transaction test", &fake_test);

    assert_frontend_checks(&frontend_source, frontend.path(), "device");
}

#[test]
#[ignore = "在临时真实 Workspace 中验证 Post 的 application 与 control DB 生成层"]
fn post_control_slice_compiles_in_temporary_real_workspace() {
    let backend_source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("生成器应位于后端 Workspace/crates")
        .to_path_buf();
    let frontend_source = backend_source
        .parent()
        .expect("后端应有工作区父目录")
        .join("ryframe-vue3");
    assert!(frontend_source.join("node_modules").is_dir());
    let backend_parent = backend_source.join(".local-tests");
    let frontend_parent = frontend_source.join(".local-tests");
    fs::create_dir_all(&backend_parent).expect("应创建后端临时测试目录");
    fs::create_dir_all(&frontend_parent).expect("应创建前端临时测试目录");
    let backend = tempfile::Builder::new()
        .prefix("post-resource-workspace-")
        .tempdir_in(&backend_parent)
        .expect("应创建 Post 临时 Workspace");
    let frontend = tempfile::Builder::new()
        .prefix("post-resource-frontend-")
        .tempdir_in(&frontend_parent)
        .expect("应创建 Post 临时前端输出目录");

    for file in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "rustfmt.toml",
    ] {
        fs::copy(backend_source.join(file), backend.path().join(file))
            .unwrap_or_else(|error| panic!("复制 {file} 失败：{error}"));
    }
    for directory in [".cargo", "catalog", "crates", "vendor", "xtask"] {
        copy_directory(
            &backend_source.join(directory),
            &backend.path().join(directory),
        );
    }
    prepare_frontend_workspace(&frontend_source, frontend.path());

    let post =
        load_resource(backend_source.join("catalog/resources/post.toml")).expect("Post 清单应有效");
    let catalog = render_resources(&[post]).expect("Post 应能生成");
    write_resource(
        &catalog,
        "post",
        ResourceWorkspace {
            backend_root: backend.path(),
            frontend_root: Some(frontend.path()),
        },
    )
    .expect("Post 应写入临时 Workspace");

    for crate_name in ["ryframe-application", "ryframe-db"] {
        let lib = backend
            .path()
            .join("crates")
            .join(crate_name)
            .join("src/lib.rs");
        let mut source = fs::read_to_string(&lib).expect("应读取 crate lib.rs");
        if !source
            .lines()
            .any(|line| line.trim() == "pub mod generated;")
        {
            source.push_str("\npub mod generated;\n");
        }
        fs::write(&lib, source).expect("应在临时副本接入 generated module");
    }

    let cargo_fmt = Command::new("cargo")
        .args(["fmt", "--all", "--", "--check"])
        .current_dir(backend.path())
        .output()
        .expect("应检查 Post 生成 Rust 资产格式");
    assert_command_succeeded("Post cargo fmt --check", &cargo_fmt);

    let cargo = Command::new("cargo")
        .args(["check", "-p", "ryframe-application", "-p", "ryframe-db"])
        .current_dir(backend.path())
        .env(
            "CARGO_TARGET_DIR",
            backend_source.join("target/resource-generator-workspace-check"),
        )
        .output()
        .expect("应运行 Post 临时后端 cargo check");
    assert_command_succeeded("Post cargo check", &cargo);
    assert_frontend_checks(&frontend_source, frontend.path(), "post");
}

fn prepare_frontend_workspace(source: &Path, target: &Path) {
    copy_directory(&source.join("src"), &target.join("src"));
    for file in ["tsconfig.json", "eslint.config.js", "package.json"] {
        fs::copy(source.join(file), target.join(file))
            .unwrap_or_else(|error| panic!("复制前端 {file} 失败：{error}"));
    }
}

fn assert_frontend_checks(source: &Path, target: &Path, resource: &str) {
    let vue_tsc = source.join("node_modules/.bin/vue-tsc.cmd");
    let typecheck = Command::new(vue_tsc)
        .args(["--noEmit", "-p", "tsconfig.json"])
        .current_dir(target)
        .output()
        .expect("应运行临时前端 vue-tsc");
    assert_command_succeeded(&format!("{resource} vue-tsc"), &typecheck);

    let eslint = source.join("node_modules/.bin/eslint.cmd");
    let generated = format!("src/generated/resources/{resource}");
    let lint = Command::new(eslint)
        .args([generated.as_str(), "--max-warnings=0"])
        .current_dir(target)
        .output()
        .expect("应运行临时前端 ESLint");
    assert_command_succeeded(&format!("{resource} ESLint"), &lint);
}

fn copy_directory(source: &Path, target: &Path) {
    fs::create_dir_all(target)
        .unwrap_or_else(|error| panic!("创建目录 {} 失败：{error}", target.display()));
    let mut entries = fs::read_dir(source)
        .unwrap_or_else(|error| panic!("读取目录 {} 失败：{error}", source.display()))
        .collect::<Result<Vec<_>, _>>()
        .expect("应读取目录项");
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type().expect("应读取文件类型").is_dir() {
            copy_directory(&source_path, &target_path);
        } else {
            fs::copy(&source_path, &target_path).unwrap_or_else(|error| {
                panic!(
                    "复制 {} 到 {} 失败：{error}",
                    source_path.display(),
                    target_path.display()
                )
            });
        }
    }
}

fn register_device_frontend_contract(frontend: &Path) {
    let operations_path = frontend.join("src/api/generated/operations.ts");
    let mut operations = fs::read_to_string(&operations_path).expect("应读取候选 operation 清单");
    let fixture = include_str!("fixtures/device_operations.ts.part");
    let (operation_ids, descriptors) = fixture
        .split_once("\n\nexport const")
        .expect("Device operation fixture 应分为 ID 和描述符");
    operations = operations.replacen(
        "export type OperationId =\n",
        &format!("export type OperationId =\n{operation_ids}\n"),
        1,
    );
    operations.push_str("\n\nexport const");
    operations.push_str(descriptors);
    fs::write(operations_path, operations).expect("应写入临时候选 operation 清单");

    let schema_path = frontend.join("src/api/generated/schema.ts");
    let mut schema = fs::read_to_string(&schema_path).expect("应读取候选 OpenAPI schema");
    let component_marker = "export interface components {\n    schemas: {\n";
    assert!(
        schema.contains(component_marker),
        "候选 schema 缺少 components 接口"
    );
    schema = schema.replacen(
        component_marker,
        &format!("{component_marker}        DeviceVo: DeviceContractRecord;\n"),
        1,
    );
    let marker = "export interface operations {\n";
    assert!(schema.contains(marker), "候选 schema 缺少 operations 接口");
    let schema = schema.replacen(marker, include_str!("fixtures/device_schema.ts.part"), 1);
    fs::write(schema_path, schema).expect("应写入临时候选 OpenAPI schema");
}

fn write_device_fake_transaction_test(backend: &Path) {
    let tests = backend.join("crates/ryframe-application/tests");
    fs::create_dir_all(&tests).expect("应创建临时 application tests");
    fs::write(
        tests.join("generated_device_fake.rs"),
        r#"use chrono::Utc;
use ryframe_application::generated::device::{
    DeviceFailure, DeviceFakePersistence, DevicePersistencePort, DeviceRecord,
};
use ryframe_application::{PersistenceTransaction, TransactionAuditMode};

fn record(id: i64) -> DeviceRecord {
    DeviceRecord {
        tenant_id: "tenant-a".into(),
        id,
        name: format!("device-{id}"),
        status: 1,
        created_at: Utc::now(),
        updated_at: None,
        del_flag: 0,
    }
}

#[tokio::test]
async fn transaction_view_only_becomes_visible_after_successful_commit() {
    let fake = DeviceFakePersistence::default();

    let rolled_back = fake.begin("tenant-a").await.unwrap();
    rolled_back.insert(record(1)).await.unwrap();
    rolled_back.rollback().await.unwrap();
    assert!(fake.find_by_id("tenant-a", 1).await.unwrap().is_none());

    let commit_failed = fake.begin("tenant-a").await.unwrap();
    commit_failed.insert(record(2)).await.unwrap();
    fake.fail_next(DeviceFailure::Commit);
    assert!(
        commit_failed
            .commit(TransactionAuditMode::Skip)
            .await
            .is_err()
    );
    assert!(fake.find_by_id("tenant-a", 2).await.unwrap().is_none());

    let first = fake.begin("tenant-a").await.unwrap();
    let second = fake.begin("tenant-a").await.unwrap();
    first.insert(record(3)).await.unwrap();
    second.insert(record(4)).await.unwrap();
    first.commit(TransactionAuditMode::Skip).await.unwrap();
    second.commit(TransactionAuditMode::Skip).await.unwrap();
    assert!(fake.find_by_id("tenant-a", 3).await.unwrap().is_some());
    assert!(fake.find_by_id("tenant-a", 4).await.unwrap().is_some());
}
"#,
    )
    .expect("应写入临时 Fake 事务测试");
}

fn assert_command_succeeded(label: &str, output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{label} 失败\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
