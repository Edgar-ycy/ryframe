use std::{
    any::Any,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use ryframe_generator::{
    RelationIr, RelationKind, ResourceWorkspace, load_resource, render_resources, write_resource,
};

static SHARED_WORKSPACE_RESULT: OnceLock<Result<(), String>> = OnceLock::new();

#[test]
#[ignore = "完整门禁在共享的临时真实 Workspace 中运行 Cargo 与 vue-tsc"]
fn device_slice_compiles_in_temporary_real_workspaces() {
    assert_shared_workspace("Device");
}

#[test]
#[ignore = "在共享的临时真实 Workspace 中验证 Post 的 application 与 control DB 生成层"]
fn post_control_slice_compiles_in_temporary_real_workspace() {
    assert_shared_workspace("Post");
}

fn assert_shared_workspace(resource: &str) {
    if let Err(error) = SHARED_WORKSPACE_RESULT.get_or_init(|| {
        std::panic::catch_unwind(run_shared_workspace)
            .map_err(panic_message)
            .and_then(|result| result)
    }) {
        panic!("{resource} 共享临时 Workspace 验证失败：{error}");
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|value| (*value).to_owned())
        })
        .unwrap_or_else(|| "共享临时 Workspace 发生未知 panic".to_owned())
}

fn run_shared_workspace() -> Result<(), String> {
    let backend_source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or("生成器应位于后端 Workspace/crates")?
        .to_path_buf();
    let frontend_source = backend_source
        .parent()
        .ok_or("后端应有工作区父目录")?
        .join("ryframe-vue3");
    if !frontend_source.join("node_modules").is_dir() {
        return Err("前端 node_modules 不存在".into());
    }

    let backend_parent = backend_source.join(".local-tests");
    let frontend_parent = frontend_source.join(".local-tests");
    fs::create_dir_all(&backend_parent).expect("应创建后端临时测试目录");
    fs::create_dir_all(&frontend_parent).expect("应创建前端临时测试目录");
    let backend = backend_parent.join("shared-resource-workspace");
    let frontend = frontend_parent.join("shared-resource-frontend");
    fs::create_dir_all(&backend).expect("应创建后端临时 Workspace");
    fs::create_dir_all(&frontend).expect("应创建前端临时 Workspace");

    for file in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "rustfmt.toml",
    ] {
        sync_file(&backend_source.join(file), &backend.join(file));
    }
    for directory in [".cargo", "catalog", "crates", "vendor", "xtask"] {
        sync_directory(&backend_source.join(directory), &backend.join(directory));
    }
    prepare_frontend_workspace(&frontend_source, &frontend);

    let mut device =
        load_resource(backend_source.join("crates/ryframe-generator/tests/fixtures/device.toml"))
            .expect("Device 清单应有效");
    device.relations.push(RelationIr {
        name: "parent".into(),
        pascal_name: "Parent".into(),
        kind: RelationKind::BelongsTo,
        local_field: "id".into(),
        target_resource: "device".into(),
        target_pascal_name: "Device".into(),
    });
    let post = load_resource(backend_source.join("catalog/resources/post.toml"))
        .expect("临时 Workspace 中既有的 Post 清单应有效");
    let catalog = render_resources(&[device, post]).expect("Device 与既有资源应能共同生成");
    let first = write_resource(
        &catalog,
        "device",
        ResourceWorkspace {
            backend_root: &backend,
            frontend_root: Some(&frontend),
        },
    )
    .expect("Device/Post 目录应一次性写入临时 Workspace");
    assert!(!first.written.is_empty(), "首次生成必须写入资产");
    for path in [
        "crates/ryframe-application/src/generated/post/service.rs",
        "crates/ryframe-db/src/generated/post/repository.rs",
        "src/generated/resources/post/page.vue",
    ] {
        let root = if path.starts_with("src/") {
            &frontend
        } else {
            &backend
        };
        assert!(root.join(path).is_file(), "Post 资产未生成：{path}");
    }
    for resource in ["device", "post"] {
        let repeated = write_resource(
            &catalog,
            resource,
            ResourceWorkspace {
                backend_root: &backend,
                frontend_root: Some(&frontend),
            },
        )
        .unwrap_or_else(|error| panic!("{resource} 连续生成应成功：{error}"));
        assert!(repeated.written.is_empty(), "{resource} 连续生成不得写入");
        assert!(repeated.removed.is_empty(), "{resource} 连续生成不得删除");
    }
    for crate_name in [
        "ryframe-application",
        "ryframe-db",
        "ryframe-tenant-db",
        "ryframe-api",
    ] {
        let lib = backend.join("crates").join(crate_name).join("src/lib.rs");
        let mut source = fs::read_to_string(&lib).expect("应读取 crate lib.rs");
        if !source
            .lines()
            .any(|line| line.trim() == "pub mod generated;")
        {
            source.push_str("\npub mod generated;\n");
        }
        fs::write(&lib, source).expect("应在临时副本接入 generated module");
    }
    register_device_frontend_contract(&frontend);
    write_device_fake_transaction_test(&backend);
    let cargo_target = std::env::var_os("RYFRAME_RESOURCE_WORKSPACE_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| backend_source.join("target/resource-generator-workspace-check"));

    let cargo_fmt = Command::new("cargo")
        .args(["fmt", "--all", "--", "--check"])
        .current_dir(&backend)
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
        .current_dir(&backend)
        .env("CARGO_TARGET_DIR", &cargo_target)
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
        .current_dir(&backend)
        .env("CARGO_TARGET_DIR", cargo_target)
        .output()
        .expect("应运行生成 Fake 事务语义测试");
    assert_command_succeeded("generated Device fake transaction test", &fake_test);

    assert_frontend_checks(&frontend_source, &frontend);
    Ok(())
}

fn prepare_frontend_workspace(source: &Path, target: &Path) {
    sync_directory(&source.join("src"), &target.join("src"));
    for file in ["tsconfig.json", "eslint.config.js", "package.json"] {
        sync_file(&source.join(file), &target.join(file));
    }
}

fn assert_frontend_checks(source: &Path, target: &Path) {
    let vue_tsc = source.join("node_modules/.bin/vue-tsc.cmd");
    let typecheck = Command::new(vue_tsc)
        .args(["--noEmit", "-p", "tsconfig.json"])
        .current_dir(target)
        .output()
        .expect("应运行临时前端 vue-tsc");
    assert_command_succeeded("Device/Post vue-tsc", &typecheck);

    let eslint = source.join("node_modules/.bin/eslint.cmd");
    let lint = Command::new(eslint)
        .args([
            "src/generated/resources/device",
            "src/generated/resources/post",
            "--max-warnings=0",
        ])
        .current_dir(target)
        .output()
        .expect("应运行临时前端 ESLint");
    assert_command_succeeded("Device/Post ESLint", &lint);
}

fn sync_directory(source: &Path, target: &Path) {
    fs::create_dir_all(target)
        .unwrap_or_else(|error| panic!("创建目录 {} 失败：{error}", target.display()));
    let source_names = fs::read_dir(source)
        .unwrap_or_else(|error| panic!("读取目录 {} 失败：{error}", source.display()))
        .map(|entry| entry.expect("应读取目录项").file_name())
        .collect::<std::collections::BTreeSet<_>>();
    for entry in fs::read_dir(target)
        .unwrap_or_else(|error| panic!("读取目录 {} 失败：{error}", target.display()))
    {
        let entry = entry.expect("应读取目标目录项");
        if source_names.contains(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        if entry.file_type().expect("应读取目标文件类型").is_dir() {
            fs::remove_dir_all(&path)
                .unwrap_or_else(|error| panic!("删除旧目录 {} 失败：{error}", path.display()));
        } else {
            fs::remove_file(&path)
                .unwrap_or_else(|error| panic!("删除旧文件 {} 失败：{error}", path.display()));
        }
    }
    let mut entries = fs::read_dir(source)
        .unwrap_or_else(|error| panic!("读取目录 {} 失败：{error}", source.display()))
        .collect::<Result<Vec<_>, _>>()
        .expect("应读取目录项");
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type().expect("应读取文件类型").is_dir() {
            sync_directory(&source_path, &target_path);
        } else {
            sync_file(&source_path, &target_path);
        }
    }
}

fn sync_file(source: &Path, target: &Path) {
    let unchanged = target.is_file()
        && fs::read(source).expect("应读取源文件") == fs::read(target).expect("应读取目标文件");
    if unchanged {
        return;
    }
    fs::copy(source, target).unwrap_or_else(|error| {
        panic!(
            "复制 {} 到 {} 失败：{error}",
            source.display(),
            target.display()
        )
    });
}

fn register_device_frontend_contract(frontend: &Path) {
    let permissions_path = frontend.join("src/api/generated/permissions.ts");
    let permissions = fs::read_to_string(&permissions_path).expect("应读取候选权限清单");
    let permissions = permissions.replacen(
        "export const permissionCatalog = [\n",
        "export const permissionCatalog = [\n  \"system:device:list\",\n",
        1,
    );
    fs::write(permissions_path, permissions).expect("应写入临时候选权限清单");

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

    let schema_path = frontend.join("src/api/generated/schema/system.ts");
    let mut schema = fs::read_to_string(&schema_path).expect("应读取候选 OpenAPI schema");
    let component_marker = "export interface components {\n    schemas: {\n";
    assert!(
        schema.contains(component_marker),
        "候选 schema 缺少 components 接口"
    );
    schema = schema.replacen(
        component_marker,
        &format!(
            "{component_marker}        DeviceVo: DeviceContractRecord;\n        DeviceDetailVo: DeviceContractRecord & {{ parent?: DeviceContractRecord | null }};\n"
        ),
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
