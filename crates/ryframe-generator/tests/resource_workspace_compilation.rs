use std::{
    any::Any,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use ryframe_generator::{
    AssetRoot, OwnershipManifest, RelationIr, RelationKind, ResourceWorkspace, load_resource,
    render_resources, write_resource,
};

static SHARED_WORKSPACE_RESULT: OnceLock<Result<(), String>> = OnceLock::new();
const FRONTEND_DIR_ENV: &str = "RYFRAME_RESOURCE_WORKSPACE_FRONTEND_DIR";
const PROFILE_ENV: &str = "RYFRAME_RESOURCE_WORKSPACE_PROFILE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerificationProfile {
    Full,
    Targeted,
}

impl VerificationProfile {
    fn from_value(value: Option<&str>) -> Result<Self, String> {
        match value {
            None => Ok(Self::Full),
            Some("targeted") => Ok(Self::Targeted),
            Some(value) => Err(format!(
                "资源 Workspace 验证模式 `{value}` 无效；仅 targeted gate 可设置 targeted"
            )),
        }
    }

    fn from_environment() -> Result<Self, String> {
        let Some(value) = std::env::var_os(PROFILE_ENV) else {
            return Self::from_value(None);
        };
        let value = value.into_string().map_err(|_| {
            format!("资源 Workspace 验证模式环境变量 {PROFILE_ENV} 不是有效 Unicode")
        })?;
        Self::from_value(Some(&value))
    }
}

struct SharedWorkspace {
    backend_source: PathBuf,
    frontend_source: PathBuf,
    backend: PathBuf,
    frontend: PathBuf,
    cargo_target: PathBuf,
}

#[test]
#[ignore = "完整门禁在共享的临时真实 Workspace 中运行 Cargo 与 vue-tsc"]
fn order_slice_compiles_in_temporary_real_workspaces() {
    assert_shared_workspace("Order");
}

#[test]
#[ignore = "在共享的临时真实 Workspace 中验证 Post 的 application 与 control DB 生成层"]
fn post_control_slice_compiles_in_temporary_real_workspace() {
    assert_shared_workspace("Post");
}

#[test]
#[ignore = "在共享的临时真实 Workspace 中验证 Notice 的标准 CRUD 生成层"]
fn notice_control_slice_compiles_in_temporary_real_workspace() {
    assert_shared_workspace("Notice");
}

#[test]
fn missing_order_frontend_contract_is_injected_once() {
    let permissions = "export const permissionCatalog = [\n]\n".to_owned();
    let operations = "import { bindJsonOperation } from '../operationRequest'\n".to_owned();
    let schema = concat!(
        "export interface components {\n    schemas: {\n    }\n}\n",
        "export interface operations {\n}\n",
    )
    .to_owned();

    let injected = prepare_order_frontend_contract(permissions, operations, schema)
        .expect("缺失的 Order 契约应可装配")
        .expect("首次装配应返回修改内容");
    assert!(
        order_frontend_contract_is_complete(&injected.0, &injected.1, &injected.2)
            .expect("装配后的契约应可判定")
    );
    assert!(
        prepare_order_frontend_contract(injected.0, injected.1, injected.2)
            .expect("重复装配应成功")
            .is_none(),
        "重复装配完整 fixture 契约必须零修改"
    );
}

#[test]
fn basic_generated_order_frontend_contract_is_upgraded_once() {
    let operation_names = [
        "delete_business_orders_by_id",
        "get_business_orders",
        "get_business_orders_by_id",
        "post_business_orders",
        "put_business_orders_by_id",
    ];
    let operations = operation_names
        .map(|name| format!("export const {name} = bindJsonOperation({{}})\n"))
        .concat();
    let schema_operations = operation_names
        .map(|name| format!("    {name}: {{}}\n"))
        .concat();
    let schema = format!(
        concat!(
            "export interface components {{\n    schemas: {{\n",
            "        ApiPageResponse_OrderVo: {{}};\n",
            "        ApiResponse_OrderVo: {{ data?: {{ id: string }} }};\n",
            "        CreateOrderDto: {{}};\n",
            "        UpdateOrderDto: {{}};\n",
            "    }};\n}}\n",
            "export interface operations {{\n{schema_operations}}}\n",
        ),
        schema_operations = schema_operations,
    );
    let permissions = "\"business:order:list\"".to_owned();

    let upgraded =
        prepare_order_frontend_contract(permissions.clone(), operations.clone(), schema)
            .expect("完整 basic generated 契约应可升级")
            .expect("首次升级应补充 OrderDetailVo");
    assert_eq!(upgraded.0, permissions, "升级不得改写已生成权限清单");
    assert_eq!(upgraded.1, operations, "升级不得改写已生成 operation");
    let order_vo_reference = "import(\"./core\").components[\"schemas\"][\"OrderVo\"]";
    assert_eq!(
        upgraded.2.matches(order_vo_reference).count(),
        2,
        "OrderDetailVo 本体与 parent 必须复用现有 OrderVo 的强类型引用"
    );
    assert_eq!(upgraded.2.matches("OrderDetailVo:").count(), 1);
    assert!(
        prepare_order_frontend_contract(upgraded.0, upgraded.1, upgraded.2)
            .expect("升级后的 generated 契约应可识别")
            .is_none(),
        "升级后的 generated 契约必须零修改"
    );
}

#[test]
fn partial_order_frontend_contract_fails_closed() {
    let partial_contracts = [
        (
            "export const permissionCatalog = [\n  \"business:order:list\",\n]\n",
            "",
            "",
        ),
        ("", "", "OrderDetailVo: {}"),
        (
            "\"business:order:list\"",
            "export const get_business_orders = bindJsonOperation({})",
            "ApiPageResponse_OrderVo:\nApiResponse_OrderVo:",
        ),
    ];
    for (permissions, operations, schema) in partial_contracts {
        assert!(
            prepare_order_frontend_contract(permissions.into(), operations.into(), schema.into())
                .is_err(),
            "部分存在的 Order 契约必须失败，不能猜测补齐"
        );
    }
}

#[test]
fn verification_profile_defaults_to_full_and_rejects_unknown_values() {
    assert_eq!(
        VerificationProfile::from_value(None).unwrap(),
        VerificationProfile::Full
    );
    assert_eq!(
        VerificationProfile::from_value(Some("targeted")).unwrap(),
        VerificationProfile::Targeted
    );
    for value in ["", "full", "targeted ", "unknown"] {
        assert!(
            VerificationProfile::from_value(Some(value)).is_err(),
            "未知模式 {value:?} 必须失败关闭"
        );
    }
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
    let profile = VerificationProfile::from_environment()?;
    let workspace = prepare_shared_workspace(profile)?;
    generate_resource_slices(&workspace, profile);
    if profile == VerificationProfile::Targeted {
        return Ok(());
    }
    register_order_capability(&workspace.backend);
    register_generated_backend_modules(&workspace.backend);
    register_order_frontend_contract(&workspace.frontend);
    write_order_fake_transaction_test(&workspace.backend);
    assert_backend_checks(&workspace);
    assert_frontend_checks(&workspace.frontend_source, &workspace.frontend);
    Ok(())
}

fn register_order_capability(backend: &Path) {
    let catalog = backend.join("catalog/access.toml");
    let mut source = fs::read_to_string(&catalog).expect("应读取临时访问目录");
    source.push_str(
        r#"

[[capabilities]]
code = "business.order"
route_keys = ["business.order"]
page_keys = ["business.order"]
permissions = ["business:order:create", "business:order:read", "business:order:list", "business:order:update", "business:order:delete"]
"#,
    );
    fs::write(catalog, source).expect("应在临时副本登记设备访问能力");

    let application =
        backend.join("crates/ryframe-application/src/system/product_capability_catalog.rs");
    let source = fs::read_to_string(&application).expect("应读取临时产品能力目录");
    let marker = "];\n\npub fn capability_descriptor";
    let capability = r#"    standard_capability(
        "business.order",
        "设备管理",
        &["business.order"],
        &[
            "business:order:create",
            "business:order:read",
            "business:order:list",
            "business:order:update",
            "business:order:delete",
        ],
        &[],
    ),
];

pub fn capability_descriptor"#;
    let source = source.replacen(marker, capability, 1);
    assert_ne!(
        source,
        fs::read_to_string(&application).unwrap(),
        "产品能力目录标记必须存在"
    );
    fs::write(application, source).expect("应在临时副本登记设备产品能力");
}

fn prepare_shared_workspace(profile: VerificationProfile) -> Result<SharedWorkspace, String> {
    let current_dir = std::env::current_dir()
        .map_err(|error| format!("无法读取资源 Workspace 当前目录：{error}"))?;
    let backend_source = current_dir
        .ancestors()
        .find(|path| {
            path.join("catalog/resources").is_dir()
                && path
                    .join("crates/ryframe-generator/tests/fixtures/order.toml")
                    .is_file()
        })
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("无法从 {} 定位后端 Workspace 根目录", current_dir.display()))?;
    let frontend_source = std::env::var_os(FRONTEND_DIR_ENV)
        .map(PathBuf::from)
        .ok_or("资源 Workspace 验证缺少前端目录环境变量")?;
    if !frontend_source.join("node_modules").is_dir() {
        return Err("前端 node_modules 不存在".into());
    }

    let backend_parent = backend_source.join(".local-tests");
    let frontend_parent = frontend_source.join(".local-tests");
    fs::create_dir_all(&backend_parent).expect("应创建后端临时测试目录");
    fs::create_dir_all(&frontend_parent).expect("应创建前端临时测试目录");
    let backend = backend_parent.join("shared-resource-workspace");
    let frontend = frontend_parent.join("shared-resource-frontend");
    reset_workspace_directory(&backend);
    reset_workspace_directory(&frontend);

    if profile == VerificationProfile::Full {
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
    } else {
        // targeted 只验证生成计划和重复写入，不运行临时 Workspace 编译；保留生成器
        // 用于识别根目录的两个标记文件即可，避免复制 crates/vendor 与完整前端源码。
        sync_file(
            &backend_source.join("Cargo.toml"),
            &backend.join("Cargo.toml"),
        );
        sync_file(
            &frontend_source.join("package.json"),
            &frontend.join("package.json"),
        );
    }

    let cargo_target = std::env::var_os("RYFRAME_RESOURCE_WORKSPACE_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| backend_source.join("target/resource-generator-workspace-check"));
    Ok(SharedWorkspace {
        backend_source,
        frontend_source,
        backend,
        frontend,
        cargo_target,
    })
}

fn reset_workspace_directory(path: &Path) {
    if path.exists() {
        fs::remove_dir_all(path)
            .unwrap_or_else(|error| panic!("清理临时 Workspace {} 失败：{error}", path.display()));
    }
    fs::create_dir_all(path)
        .unwrap_or_else(|error| panic!("创建临时 Workspace {} 失败：{error}", path.display()));
}

fn generate_resource_slices(workspace: &SharedWorkspace, profile: VerificationProfile) {
    let mut order = load_resource(
        workspace
            .backend_source
            .join("crates/ryframe-generator/tests/fixtures/order.toml"),
    )
    .expect("Order 清单应有效");
    order.relations.push(RelationIr {
        name: "parent".into(),
        pascal_name: "Parent".into(),
        kind: RelationKind::BelongsTo,
        local_field: "id".into(),
        target_resource: "order".into(),
        target_pascal_name: "Order".into(),
    });
    let post = load_resource(workspace.backend_source.join("catalog/resources/post.toml"))
        .expect("临时 Workspace 中既有的 Post 清单应有效");
    let notice = load_resource(
        workspace
            .backend_source
            .join("catalog/resources/notice.toml"),
    )
    .expect("临时 Workspace 中既有的 Notice 清单应有效");
    let catalog = render_resources(&[order, notice, post]).expect("Order 与既有资源应能共同生成");
    let initial_resources: &[&str] = if profile == VerificationProfile::Targeted {
        &["notice", "post", "order"]
    } else {
        &["order"]
    };
    for resource in initial_resources {
        let first = write_resource(
            &catalog,
            resource,
            ResourceWorkspace {
                backend_root: &workspace.backend,
                frontend_root: Some(&workspace.frontend),
            },
        )
        .unwrap_or_else(|error| panic!("{resource} 首次生成应成功：{error}"));
        assert!(!first.written.is_empty(), "{resource} 首次生成必须写入资产");
    }
    assert_generated_ownership(workspace);
    for path in [
        "crates/ryframe-application/src/generated/post/service.rs",
        "crates/ryframe-application/src/generated/notice/service.rs",
        "crates/ryframe-api/src/generated/notice/handler.rs",
        "crates/ryframe-db/src/generated/post/repository.rs",
        "crates/ryframe-db/src/generated/notice/repository.rs",
        "src/generated/resources/post/page.vue",
        "src/generated/resources/notice/page.vue",
        "src/generated/resources/notice/registration.ts",
    ] {
        let root = if path.starts_with("src/") {
            &workspace.frontend
        } else {
            &workspace.backend
        };
        assert!(root.join(path).is_file(), "Post 资产未生成：{path}");
    }
    for resource in ["order", "notice", "post"] {
        let repeated = write_resource(
            &catalog,
            resource,
            ResourceWorkspace {
                backend_root: &workspace.backend,
                frontend_root: Some(&workspace.frontend),
            },
        )
        .unwrap_or_else(|error| panic!("{resource} 连续生成应成功：{error}"));
        assert!(repeated.written.is_empty(), "{resource} 连续生成不得写入");
        assert!(repeated.removed.is_empty(), "{resource} 连续生成不得删除");
    }
    let notice_registration = fs::read_to_string(
        workspace
            .frontend
            .join("src/generated/resources/notice/registration.ts"),
    )
    .expect("应读取 Notice 页面注册清单");
    assert!(
        notice_registration.contains("@/views/system/notice/index.vue"),
        "Notice 必须保留强类型自定义页面扩展"
    );
}

fn assert_generated_ownership(workspace: &SharedWorkspace) {
    let manifest_path = workspace.backend.join("catalog/resources/.ownership.toml");
    let manifest = fs::read_to_string(&manifest_path).expect("应读取生成后的 ownership manifest");
    let manifest: OwnershipManifest = toml::from_str(&manifest).expect("ownership manifest 应有效");
    let selected = ["order", "notice", "post"];
    let mut roots = std::collections::BTreeSet::new();
    let mut paths = std::collections::BTreeSet::new();
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| selected.contains(&entry.resource.as_str()))
    {
        assert!(
            paths.insert((entry.root, entry.path.as_str())),
            "ownership 不得重复登记路径：{}",
            entry.path
        );
        let root = match entry.root {
            AssetRoot::Backend => &workspace.backend,
            AssetRoot::Frontend => &workspace.frontend,
        };
        assert!(
            root.join(&entry.path).is_file(),
            "ownership 资产不存在：{}",
            entry.path
        );
        roots.insert((entry.resource.as_str(), entry.root));
    }
    for resource in selected {
        for root in [AssetRoot::Backend, AssetRoot::Frontend] {
            assert!(
                roots.contains(&(resource, root)),
                "{resource} 缺少 {} ownership",
                root.label()
            );
        }
    }
}

fn register_generated_backend_modules(backend: &Path) {
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
}

fn assert_backend_checks(workspace: &SharedWorkspace) {
    let cargo_fmt = Command::new("cargo")
        .args(["fmt", "--all", "--", "--check"])
        .current_dir(&workspace.backend)
        .output()
        .expect("应检查生成 Rust 资产格式");
    assert_command_succeeded("cargo fmt --check", &cargo_fmt);

    let migration = Command::new("cargo")
        .args([
            "check",
            "-p",
            "ryframe-db",
            "-p",
            "ryframe-tenant-db",
            "--no-default-features",
            "--features",
            "ryframe-db/migration,ryframe-tenant-db/migration",
        ])
        .current_dir(&workspace.backend)
        .env("CARGO_TARGET_DIR", &workspace.cargo_target)
        .output()
        .expect("应运行生成迁移最小面 cargo check");
    assert_command_succeeded("cargo check generated migrations", &migration);

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
            "--no-default-features",
            "--features",
            "ryframe-db/repositories,ryframe-tenant-db/repositories",
        ])
        .current_dir(&workspace.backend)
        .env("CARGO_TARGET_DIR", &workspace.cargo_target)
        .output()
        .expect("应运行临时后端 cargo check");
    assert_command_succeeded("cargo check generated repositories", &cargo);

    let fake_test = Command::new("cargo")
        .args([
            "test",
            "-p",
            "ryframe-application",
            "--no-default-features",
            "--features",
            "test-support",
            "--test",
            "generated_order_fake",
        ])
        .current_dir(&workspace.backend)
        .env("CARGO_TARGET_DIR", &workspace.cargo_target)
        .output()
        .expect("应运行生成 Fake 事务语义测试");
    assert_command_succeeded("generated Order fake transaction test", &fake_test);
}

fn prepare_frontend_workspace(source: &Path, target: &Path) {
    sync_directory(&source.join("src"), &target.join("src"));
    for file in [
        "tsconfig.json",
        "tsconfig.base.json",
        "tsconfig.app.json",
        "eslint.config.js",
        "package.json",
    ] {
        sync_file(&source.join(file), &target.join(file));
    }
}

fn assert_frontend_checks(source: &Path, target: &Path) {
    let vue_tsc = source.join("node_modules/vue-tsc/bin/vue-tsc.js");
    let typecheck = Command::new("node")
        .arg(vue_tsc)
        .args(["--noEmit", "-p", "tsconfig.app.json"])
        .current_dir(target)
        .output()
        .expect("应运行临时前端 vue-tsc");
    assert_command_succeeded("Order/Notice/Post vue-tsc", &typecheck);

    let eslint = source.join("node_modules/eslint/bin/eslint.js");
    let lint = Command::new("node")
        .arg(eslint)
        .args([
            "src/generated/resources/order",
            "src/generated/resources/notice",
            "src/generated/resources/post",
            "--max-warnings=0",
        ])
        .current_dir(target)
        .output()
        .expect("应运行临时前端 ESLint");
    assert_command_succeeded("Order/Notice/Post ESLint", &lint);
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

fn register_order_frontend_contract(frontend: &Path) {
    let permissions_path = frontend.join("src/api/generated/permissions.ts");
    let permissions = fs::read_to_string(&permissions_path).expect("应读取候选权限清单");
    let operations_path = frontend.join("src/api/generated/operations/system.ts");
    let operations = fs::read_to_string(&operations_path).expect("应读取候选 operation 清单");
    let schema_path = frontend.join("src/api/generated/schema/system.ts");
    let schema = fs::read_to_string(&schema_path).expect("应读取候选 OpenAPI schema");

    let Some((permissions, operations, schema)) =
        prepare_order_frontend_contract(permissions, operations, schema)
            .unwrap_or_else(|error| panic!("候选 Order 前端契约不完整：{error}"))
    else {
        return;
    };
    fs::write(permissions_path, permissions).expect("应写入临时候选权限清单");
    fs::write(operations_path, operations).expect("应写入临时候选 operation 清单");
    fs::write(schema_path, schema).expect("应写入临时候选 OpenAPI schema");
}

fn prepare_order_frontend_contract(
    permissions: String,
    mut operations: String,
    mut schema: String,
) -> Result<Option<(String, String, String)>, String> {
    match order_frontend_contract_state(&permissions, &operations, &schema)? {
        OrderFrontendContractState::Complete => return Ok(None),
        OrderFrontendContractState::GeneratedBasic => {
            add_generated_order_detail_schema(&mut schema)?;
            debug_assert!(
                order_frontend_contract_is_complete(&permissions, &operations, &schema)
                    .expect("刚升级的 Order 前端契约应完整")
            );
            return Ok(Some((permissions, operations, schema)));
        }
        OrderFrontendContractState::Missing => {}
    }

    let permission_marker = "export const permissionCatalog = [\n";
    if !permissions.contains(permission_marker) {
        return Err("权限清单缺少 permissionCatalog".into());
    }
    let permissions = permissions.replacen(
        permission_marker,
        "export const permissionCatalog = [\n  \"business:order:list\",\n",
        1,
    );

    operations.push('\n');
    operations.push_str(include_str!("fixtures/order_operations.ts.part"));

    let component_marker = "export interface components {\n    schemas: {\n";
    if !schema.contains(component_marker) {
        return Err("OpenAPI schema 缺少 components 接口".into());
    }
    schema = schema.replacen(
        component_marker,
        &format!(
            "{component_marker}        OrderVo: OrderContractRecord;\n        OrderDetailVo: OrderContractRecord & {{ parent?: OrderContractRecord | null }};\n"
        ),
        1,
    );
    let marker = "export interface operations {\n";
    if !schema.contains(marker) {
        return Err("OpenAPI schema 缺少 operations 接口".into());
    }
    let schema = schema.replacen(marker, include_str!("fixtures/order_schema.ts.part"), 1);
    debug_assert!(
        order_frontend_contract_is_complete(&permissions, &operations, &schema)
            .expect("刚装配的 Order 前端契约应完整")
    );
    Ok(Some((permissions, operations, schema)))
}

fn add_generated_order_detail_schema(schema: &mut String) -> Result<(), String> {
    let component_marker = "export interface components {\n    schemas: {\n";
    if schema.matches(component_marker).count() != 1 {
        return Err("OpenAPI schema 必须包含唯一的 components 接口".into());
    }
    let detail_schema = concat!(
        "        OrderDetailVo: import(\"./core\").components[\"schemas\"][\"OrderVo\"] & {\n",
        "            parent?: import(\"./core\").components[\"schemas\"][\"OrderVo\"] | null;\n",
        "        };\n",
    );
    *schema = schema.replacen(
        component_marker,
        &format!("{component_marker}{detail_schema}"),
        1,
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OrderFrontendContractState {
    Missing,
    GeneratedBasic,
    Complete,
}

fn order_frontend_contract_is_complete(
    permissions: &str,
    operations: &str,
    schema: &str,
) -> Result<bool, String> {
    Ok(matches!(
        order_frontend_contract_state(permissions, operations, schema)?,
        OrderFrontendContractState::Complete
    ))
}

fn order_frontend_contract_state(
    permissions: &str,
    operations: &str,
    schema: &str,
) -> Result<OrderFrontendContractState, String> {
    const OPERATION_NAMES: [&str; 5] = [
        "delete_business_orders_by_id",
        "get_business_orders",
        "get_business_orders_by_id",
        "post_business_orders",
        "put_business_orders_by_id",
    ];
    const GENERATED_SCHEMA_MARKERS: [&str; 4] = [
        "ApiPageResponse_OrderVo:",
        "ApiResponse_OrderVo:",
        "CreateOrderDto:",
        "UpdateOrderDto:",
    ];
    const FIXTURE_SCHEMA_MARKERS: [&str; 3] = [
        "type OrderContractRecord = {",
        "OrderVo: OrderContractRecord;",
        "OrderDetailVo: OrderContractRecord & {",
    ];
    const GENERATED_DETAIL_MARKER: &str = "OrderDetailVo:";

    let permission_present = permissions.contains("\"business:order:list\"");
    let operation_constants =
        OPERATION_NAMES.map(|name| operations.contains(&format!("export const {name} =")));
    let schema_operations = OPERATION_NAMES.map(|name| schema.contains(&format!("    {name}: {{")));
    let generated_schema = GENERATED_SCHEMA_MARKERS.map(|marker| schema.contains(marker));
    let fixture_schema = FIXTURE_SCHEMA_MARKERS.map(|marker| schema.contains(marker));

    let common_complete = permission_present
        && operation_constants.iter().all(|present| *present)
        && schema_operations.iter().all(|present| *present);
    let generated_complete = generated_schema.iter().all(|present| *present);
    let generated_any = generated_schema.iter().any(|present| *present);
    let fixture_complete = fixture_schema.iter().all(|present| *present);
    let fixture_any = fixture_schema.iter().any(|present| *present);
    let generated_detail_present = schema.contains(GENERATED_DETAIL_MARKER);
    if common_complete && fixture_complete && !generated_any {
        return Ok(OrderFrontendContractState::Complete);
    }
    if common_complete && generated_complete && !fixture_any {
        return Ok(if generated_detail_present {
            OrderFrontendContractState::Complete
        } else {
            OrderFrontendContractState::GeneratedBasic
        });
    }

    let any_present = permission_present
        || operation_constants.iter().any(|present| *present)
        || schema_operations.iter().any(|present| *present)
        || generated_any
        || fixture_any
        || generated_detail_present;
    if any_present {
        return Err("只发现部分权限、operation 或 schema 标记，拒绝猜测并重复装配".into());
    }
    Ok(OrderFrontendContractState::Missing)
}

fn write_order_fake_transaction_test(backend: &Path) {
    let tests = backend.join("crates/ryframe-application/tests");
    fs::create_dir_all(&tests).expect("应创建临时 application tests");
    fs::write(
        tests.join("generated_order_fake.rs"),
        r#"use chrono::Utc;
use ryframe_application::TransactionAuditMode;
use ryframe_application::generated::order::{
    OrderFailure, OrderFakePersistence, OrderPersistencePort, OrderRecord,
};

fn record(id: i64) -> OrderRecord {
    OrderRecord {
        tenant_id: "tenant-a".into(),
        id,
        name: format!("order-{id}"),
        status: 1,
        created_at: Utc::now(),
        updated_at: None,
        del_flag: 0,
    }
}

#[tokio::test]
async fn transaction_view_only_becomes_visible_after_successful_commit() {
    let fake = OrderFakePersistence::default();

    let rolled_back = fake.begin("tenant-a").await.unwrap();
    rolled_back.insert(record(1)).await.unwrap();
    rolled_back.rollback().await.unwrap();
    assert!(fake.find_by_id("tenant-a", 1).await.unwrap().is_none());

    let commit_failed = fake.begin("tenant-a").await.unwrap();
    commit_failed.insert(record(2)).await.unwrap();
    fake.fail_next(OrderFailure::Commit);
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
