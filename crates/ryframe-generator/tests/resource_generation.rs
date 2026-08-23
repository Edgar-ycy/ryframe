use std::{fs, path::PathBuf};

use ryframe_generator::{
    AssetRoot, GeneratedAsset, PlanAction, ResourceSpec, ResourceWorkspace, load_resource,
    normalize_resource, plan_resource_assets, plan_resource_changes, render_resources,
    write_resource, write_resources,
};

fn device_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/device.toml")
}

fn post_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../catalog/resources/post.toml")
}

fn device() -> ryframe_generator::ResourceIr {
    load_resource(device_path()).expect("Device 资源清单应有效")
}

#[test]
fn post_manifest_preserves_the_existing_public_contract_and_extensions() {
    let post = load_resource(post_path()).expect("Post 资源清单应有效");

    assert_eq!(post.table, "sys_post");
    assert_eq!(post.primary_key, ["id"]);
    assert_eq!(post.api.path, "/api/v1/system/posts");
    assert_eq!(post.api.operations.list, "get_system_posts");
    assert_eq!(post.api.operations.create, "post_system_posts");
    let id = post
        .fields
        .iter()
        .find(|field| field.name == "id")
        .expect("Post 应包含 id");
    assert_eq!(id.rust_type, "i64");
    assert_eq!(id.typescript_type, "string");
    assert_eq!(post.access.permissions.create, "system:post:add");
    assert_eq!(post.access.permissions.update, "system:post:edit");
    assert_eq!(post.access.permissions.delete, "system:post:remove");
    assert_eq!(
        post.backend_extensions.get("export_permission"),
        Some(&toml::Value::String("system:post:export".into()))
    );
    assert_eq!(
        post.frontend_extensions.get("export"),
        Some(&toml::Value::String("post".into()))
    );
}

#[test]
fn post_slice_preserves_control_configuration_and_conflict_semantics() {
    let post = load_resource(post_path()).expect("Post 资源清单应有效");
    let generated = render_resources(&[post]).expect("Post 应能生成");
    let content = |suffix: &str| {
        generated
            .assets
            .iter()
            .find(|asset| asset.path.ends_with(suffix))
            .unwrap_or_else(|| panic!("缺少生成资产 {suffix}"))
            .content
            .as_str()
    };
    let service = content("post/service.rs");
    assert!(service.contains("transaction.lock_configuration(tenant_id).await?"));
    assert!(service.contains("find_by_code_for_update"));
    assert!(service.contains("岗位编码已存在"));
    assert!(service.contains("increment_configuration_version(tenant_id)"));
    assert!(service.contains("TransactionAuditMode::CurrentRequest"));
    assert!(service.contains("command.sort.unwrap_or(0_i32)"));
    assert!(service.contains("AppError::NotFound(\"岗位不存在\".into())"));
    assert!(!service.contains("AppError::NotFound(format!"));
    let repository = content("post/repository.rs");
    assert!(repository.contains("lock_tenant_configuration_in_txn"));
    assert!(repository.contains("increment_configuration_version_in_txn"));
    assert!(repository.contains("order_by_asc(entity::Column::Sort)"));
    assert!(repository.contains("order_by_asc(entity::Column::Id)"));
    assert!(repository.contains("normalized.contains(\"uk_tenant_code\")"));
    assert!(repository.contains("岗位编码已存在"));
    assert!(repository.contains("entity::Column::Name.contains(value)"));
    assert!(repository.contains("filter.status.filter(|value| !value.is_empty())"));
    assert!(repository.contains("entity::Column::Status.eq(value)"));
    assert!(!repository.contains("entity::Column::Status.contains(value)"));
    assert!(repository.contains("active.updated_at = Set(chrono::Utc::now())"));
    assert!(repository.contains("struct DatabasePostTransaction"));
    assert!(repository.contains("tenant_id: String"));
    assert!(repository.contains("tenant_id: tenant_id.to_owned()"));
    assert!(repository.contains("self.ensure_tenant(tenant_id)?"));
    assert!(repository.contains("self.ensure_tenant(&record.tenant_id)?"));
    assert!(repository.contains("AppError::Authorization(\"岗位事务租户不匹配\".into())"));
    let entity = content("post/entity.rs");
    assert!(entity.contains("pub const SOFT_DELETE_ACTIVE: &str = \"0\";"));
    assert!(entity.contains("pub const SOFT_DELETE_DELETED: &str = \"2\";"));
    let database_mod = generated
        .assets
        .iter()
        .find(|asset| asset.path == "crates/ryframe-db/src/generated/mod.rs")
        .expect("应生成 control DB 聚合入口");
    assert!(
        database_mod
            .content
            .contains("pub use super::post::entity as post;")
    );
    let fake = content("post/fake.rs");
    assert!(fake.contains("LockConfiguration"));
    assert!(fake.contains("FindByCode"));
    assert!(fake.contains("IncrementConfigurationVersion"));
    assert!(fake.contains("view: Mutex<BTreeMap<i64, PostRecord>>"));
    assert!(fake.contains("self.lock_view().insert"));
    assert!(fake.contains("self.original.keys().filter"));
    assert!(fake.contains("self.original.get(&id) != Some(&record)"));
    assert!(!fake.contains("state.records.retain"));
    assert!(fake.contains("!record.name.contains(value)"));
    assert!(fake.contains("record.status != value"));
    assert!(fake.contains("filter.status.filter(|value| !value.is_empty())"));
    assert!(fake.contains(".filter(|((owner, _), _)| owner == tenant_id)"));
    assert!(fake.contains(".filter(|record| record.del_flag == \"0\")"));
    assert!(fake.contains("record.del_flag != \"0\""));
    assert!(fake.contains("left.sort"));
    assert!(fake.contains(".cmp(&right.sort)"));
    assert!(fake.contains(".then_with(|| left.id.cmp(&right.id))"));
    assert!(fake.contains("self.ensure_tenant(tenant_id)?"));
    assert!(fake.contains("self.ensure_tenant(&record.tenant_id)?"));
    assert!(fake.contains("AppError::Authorization(\"岗位事务租户不匹配\".into())"));
    assert!(!fake.contains(".filter_map(|((owner, id), record)|"));
    let page = content("post/page.vue");
    assert!(page.contains("name=\"actions\""));
    assert!(page.contains(":last-successful-query=\"lastSuccessfulQuery ?? null\""));
    assert!(page.contains("lastSuccessfulQuery: PostQuery | null"));
    assert!(page.contains("actions?(props:"));
    assert!(page.contains("<template v-if=\"slots.actions\" #actions>"));
    assert!(page.contains("import { computed } from 'vue'"));
    let access = generated
        .assets
        .iter()
        .find(|asset| asset.path == "catalog/access.generated.toml")
        .expect("应生成访问目录");
    assert!(!access.content.ends_with("\n\n"));
    let commit_failure = fake
        .find("fail_if_requested(&mut state, PostFailure::Commit)?")
        .expect("commit 必须先处理失败");
    let commit_apply = fake
        .find("for id in self.original.keys()")
        .expect("成功 commit 才应用事务视图");
    assert!(commit_failure < commit_apply);
    let rollback = fake
        .split("async fn rollback")
        .nth(1)
        .expect("应生成 rollback");
    assert!(!rollback.contains("state.records.insert"));
    let fields = content("post/fields.ts");
    assert!(fields.contains("disabledOnEdit: true"));
    assert!(fields.contains("editOnly: true"));
    assert!(fields.contains("String(record.name)"));
    assert!(fields.contains("const queryFields = [\n    {\n      key:"));
    assert!(!fields.contains("{ key: \"name\", kind:"));
    let api = content("post/api.ts");
    assert!(api.contains("ApiSchema, OperationJsonBody, OperationQuery"));
    assert!(api.contains("export type PostRecord = ApiSchema<'PostVo'>"));
    assert!(api.contains("export type PostQuery = OperationQuery<'get_system_posts'>"));
    assert!(api.contains("OperationJsonBody<'post_system_posts'>"));
    assert!(!api.contains("export interface PostRecord"));
    let handler = content("post/handler.rs");
    assert!(handler.contains("tag = \"岗位管理\""));
    assert!(!handler.contains("tag = \"岗位\","));
    assert!(
        generated
            .assets
            .iter()
            .all(|asset| { asset.path != "crates/ryframe-db/src/generated/post/migration.rs" })
    );
}

#[test]
fn device_manifest_normalizes_to_stable_ir() {
    let resource = device();

    assert_eq!(resource.name, "device");
    assert!(resource.source_path.ends_with("tests/fixtures/device.toml"));
    assert!(resource.bootstrap_migration);
    assert_eq!(resource.source_hash.len(), 64);
    assert_eq!(resource.primary_key, ["tenant_id", "id"]);
    assert_eq!(
        resource
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        [
            "tenant_id",
            "id",
            "name",
            "status",
            "created_at",
            "updated_at",
            "del_flag"
        ]
    );
    assert_eq!(
        resource
            .indexes
            .iter()
            .map(|index| index.name.as_str())
            .collect::<Vec<_>>(),
        ["idx_device_tenant_status", "uk_device_tenant_name"]
    );
}

#[test]
fn rendering_is_deterministic_readable_and_split_by_responsibility() {
    let resource = device();
    let first = render_resources(std::slice::from_ref(&resource)).expect("首次生成应成功");
    let second = render_resources(&[resource]).expect("再次生成应成功");

    assert_eq!(first.assets, second.assets);
    for expected in [
        "crates/ryframe-application/src/generated/device/model.rs",
        "crates/ryframe-application/src/generated/device/port.rs",
        "crates/ryframe-application/src/generated/device/service.rs",
        "crates/ryframe-application/src/generated/device/fake.rs",
        "crates/ryframe-tenant-db/src/generated/device/entity.rs",
        "crates/ryframe-tenant-db/src/generated/device/repository.rs",
        "crates/ryframe-api/src/generated/device/handler.rs",
        "crates/ryframe-api/src/generated/device/dto.rs",
        "crates/ryframe-api/src/generated/device/openapi.rs",
        "crates/ryframe-api/src/generated/crud_resources.rs",
        "crates/ryframe-tenant-db/src/generated/device/migration.rs",
        "catalog/access.generated.toml",
        "src/generated/resources/device/api.ts",
        "src/generated/resources/device/fields.ts",
        "src/generated/resources/device/page.vue",
    ] {
        assert!(
            first.assets.iter().any(|asset| asset.path == expected),
            "缺少生成资产 {expected}"
        );
    }
    for asset in &first.assets {
        assert!(asset.content.contains("@generated by RyFrame"));
        assert!(asset.content.lines().count() <= 500, "{} 过长", asset.path);
        if asset.path.ends_with(".rs") {
            syn::parse_file(&asset.content)
                .unwrap_or_else(|error| panic!("{} 语法无效：{error}", asset.path));
        }
    }

    let service = first
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("device/service.rs"))
        .expect("应生成具体服务");
    assert!(service.content.contains("pub struct DeviceService"));
    assert!(!service.content.contains("UseCase<"));
    let port = first
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("device/port.rs"))
        .expect("应生成异步端口");
    assert!(port.content.contains("#[async_trait]"));
    assert!(!port.content.contains("PersistenceFuture"));
    assert!(!port.content.contains("Box::pin"));
    let fake = first
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("device/fake.rs"))
        .expect("应生成类型化测试替身");
    assert!(fake.content.contains("pub enum DeviceCall"));
    assert!(fake.content.contains("DeviceTransactionState"));
    assert!(!fake.content.contains("Vec<String>"));
    let migration = first
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("device/migration.rs"))
        .expect("Device fixture 应生成资源首次引入迁移");
    assert!(migration.content.contains("m_resource_initial_device"));
    assert!(migration.content.contains("INITIAL_RESOURCE_MIGRATION"));
    assert!(migration.content.contains("schema-sha256:"));
    assert!(migration.content.contains("`tenant_id` VARCHAR(64)"));
    assert!(!migration.content.contains("REFERENCES `sys_tenant`"));
    assert!(migration.content.contains("禁止生产 down"));

    let repository = first
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("device/repository.rs"))
        .expect("应生成 Device repository");
    assert!(
        repository
            .content
            .contains("normalized.contains(\"uk_device_tenant_name\")")
    );
    assert!(repository.content.contains("设备名称已存在"));
    assert!(
        repository
            .content
            .contains("active.updated_at = Set(Some(chrono::Utc::now()))")
    );

    for forbidden in [
        "async fn list_device() {}",
        "IndexDescriptor",
        "MigrationDescriptor",
    ] {
        assert!(
            first
                .assets
                .iter()
                .all(|asset| !asset.content.contains(forbidden)),
            "生成资产仍包含旧骨架 {forbidden}"
        );
    }

    let page = first
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("device/page.vue"))
        .expect("应生成薄页面");
    assert!(page.content.contains("@/components/business/flat-crud"));
    assert!(!page.content.contains("@/components/FlatCrudPage"));
    let frontend_api = first
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("device/api.ts"))
        .expect("应生成强类型 API adapter");
    assert!(frontend_api.content.contains("requestOperation"));
    assert!(frontend_api.content.contains("@/api/generated/operations"));
    assert!(!frontend_api.content.contains("url:"));
    for operation in [
        "get_system_devices",
        "get_system_devices_by_id",
        "post_system_devices",
        "put_system_devices_by_id",
        "delete_system_devices_by_id",
    ] {
        assert!(frontend_api.content.contains(operation));
    }

    let aggregate = first
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("generated/crud_resources.rs"))
        .expect("应生成 CRUD OpenAPI 聚合");
    for expected in [
        "CRUD_RESOURCES_VERSION: u16 = 1",
        "crud_resources_extension",
        "super::device::openapi::crud_resource_metadata()",
        "\"resources\"",
    ] {
        assert!(aggregate.content.contains(expected), "聚合缺少 {expected}");
    }
    assert!(!aggregate.content.contains("toml::"));
    assert!(!aggregate.content.contains("read_to_string"));
    let api_mod = first
        .assets
        .iter()
        .find(|asset| asset.path == "crates/ryframe-api/src/generated/mod.rs")
        .expect("API 聚合入口应存在");
    assert!(api_mod.content.contains("pub mod crud_resources;"));
    assert!(
        api_mod
            .content
            .contains("pub use crud_resources::crud_resources_extension;")
    );

    let metadata = first
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("device/openapi.rs"))
        .expect("应生成资源 OpenAPI 元数据");
    assert!(metadata.content.contains("\"profile\": \"flat_crud\""));
    assert!(metadata.content.contains("\"fields\""));
    assert!(!metadata.content.contains("biz_device"));
}

#[test]
fn empty_catalog_generates_stable_compile_ready_aggregates() {
    let first = render_resources(&[]).expect("空资源目录仍应生成中央聚合入口");
    let second = render_resources(&[]).expect("空目录重复生成应稳定");
    assert_eq!(first.assets, second.assets);
    assert!(first.explanations.is_empty());
    for expected in [
        "crates/ryframe-application/src/generated/mod.rs",
        "crates/ryframe-application/src/generated/services.rs",
        "crates/ryframe-db/src/generated/mod.rs",
        "crates/ryframe-tenant-db/src/generated/mod.rs",
        "crates/ryframe-api/src/generated/mod.rs",
        "crates/ryframe-api/src/generated/router.rs",
        "crates/ryframe-api/src/generated/openapi.rs",
        "crates/ryframe-api/src/generated/crud_resources.rs",
        "src/generated/resources/index.ts",
        "catalog/access.generated.toml",
    ] {
        assert!(
            first.assets.iter().any(|asset| asset.path == expected),
            "空目录缺少 {expected}"
        );
    }
    for asset in first
        .assets
        .iter()
        .filter(|asset| asset.path.ends_with(".rs"))
    {
        syn::parse_file(&asset.content)
            .unwrap_or_else(|error| panic!("空聚合 {} 语法无效：{error}", asset.path));
    }
    let openapi = first
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("generated/openapi.rs"))
        .expect("应生成空 OpenAPI");
    assert!(openapi.content.contains("OpenApi::default()"));
    let metadata = first
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("generated/crud_resources.rs"))
        .expect("应生成空资源元数据");
    assert!(metadata.content.contains("\"resources\": ["));
}

#[test]
fn explanation_shows_the_complete_debugging_chain() {
    let device = device();
    let post = load_resource(post_path()).expect("Post 清单应有效");
    let generated = render_resources(&[device, post]).expect("生成应成功");
    let explanation = generated
        .explanation("device")
        .expect("应生成 explain 模型");
    let text = explanation.render_text();

    for expected in [
        "页面",
        "API adapter",
        "Handler",
        "Service",
        "Port",
        "Repository",
        "Table",
        "biz_device",
        "system:device:list",
    ] {
        assert!(text.contains(expected), "explain 缺少 {expected}");
    }
    assert!(text.contains("crates/ryframe-tenant-db/src/generated/device/repository.rs"));
    assert!(text.contains("symbol") || text.contains("router"));
    let post_explanation = generated
        .explanation("post")
        .expect("应生成 Post explain 模型");
    assert_eq!(
        post_explanation.permissions,
        [
            "system:post:add",
            "system:post:list",
            "system:post:edit",
            "system:post:remove",
        ]
    );
    let post_text = post_explanation.render_text();
    assert!(post_text.contains("crates/ryframe-db/src/generated/post/repository.rs"));
    assert!(!post_text.contains("ryframe-tenant-db/src/generated/post"));
}

#[test]
fn manifest_rejects_hidden_business_dsl_with_actionable_error() {
    let source = fs::read_to_string(device_path()).expect("应读取 fixture");
    let invalid =
        format!("{source}\n[extensions.backend.report]\nsql = \"select * from secret\"\n");
    let error = ResourceSpec::parse(&invalid, "catalog/resources/device.toml")
        .expect_err("任意 SQL 必须被拒绝")
        .to_string();

    assert!(error.contains("sql"));
    assert!(error.contains("强类型手写扩展"));
    assert!(error.contains("catalog/resources/device.toml"));
}

#[test]
fn field_errors_include_resource_field_file_and_fix() {
    let source = fs::read_to_string(device_path()).expect("应读取 fixture");
    let invalid = source.replacen("min_length = 1", "min_length = 101", 1);
    let spec =
        ResourceSpec::parse(&invalid, "catalog/resources/device.toml").expect("TOML 结构仍应有效");
    let error = normalize_resource(spec, "catalog/resources/device.toml", "fixture")
        .expect_err("错误的长度边界必须被拒绝")
        .to_string();

    for expected in [
        "资源 device",
        "字段 name",
        "catalog/resources/device.toml",
        "修复建议",
    ] {
        assert!(
            error.contains(expected),
            "错误缺少上下文 {expected}: {error}"
        );
    }
}

#[test]
fn nullable_string_enum_filter_generates_type_safe_fake_comparison() {
    let source = fs::read_to_string(post_path()).expect("应读取 Post 清单");
    let nullable = source
        .replacen(
            "name = \"status\"\nvalue_type = \"string\"",
            "name = \"status\"\nvalue_type = \"string\"\nnullable = true",
            1,
        )
        .replacen(
            "[fields.validation]\nrequired = true\n\n[fields.labels]\nzh_cn = \"状态\"",
            "[fields.labels]\nzh_cn = \"状态\"",
            1,
        );
    let spec = ResourceSpec::parse(&nullable, "catalog/resources/post.toml").expect("TOML 应有效");
    let resource = normalize_resource(spec, "catalog/resources/post.toml", "fixture")
        .expect("可空字符串枚举应能规范化");
    let generated = render_resources(&[resource]).expect("可空字符串枚举应能生成");
    let fake = generated
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("post/fake.rs"))
        .expect("应生成 Fake");

    assert!(
        fake.content
            .contains("record.status.as_deref() != Some(value)")
    );
}

#[test]
fn filtered_string_enum_rejects_empty_sentinel_key() {
    let source = fs::read_to_string(post_path()).expect("应读取 Post 清单");
    let invalid = source.replacen(
        "[fields.enum_values.\"0\"]",
        "[fields.enum_values.\"\"]\nzh_cn = \"未指定\"\nen = \"Unspecified\"\n\n[fields.enum_values.\"0\"]",
        1,
    );
    let spec = ResourceSpec::parse(&invalid, "catalog/resources/post.toml").expect("TOML 应有效");
    let error = normalize_resource(spec, "catalog/resources/post.toml", "fixture")
        .expect_err("空字符串会与未筛选哨兵冲突")
        .to_string();

    assert!(error.contains("空字符串键"));
    assert!(error.contains("未筛选"));
}

#[test]
fn safe_writer_is_idempotent_and_detects_manual_edits() {
    let backend = tempfile::tempdir().expect("应创建后端临时工作区");
    let frontend = tempfile::tempdir().expect("应创建前端临时工作区");
    fs::write(backend.path().join("Cargo.toml"), "[workspace]\n").expect("应创建工作区标识");
    let catalog = render_resources(&[device()]).expect("生成应成功");
    let workspace = ResourceWorkspace {
        backend_root: backend.path(),
        frontend_root: Some(frontend.path()),
    };

    let first = write_resources(&catalog, workspace).expect("首次写入应成功");
    assert!(!first.written.is_empty());
    assert!(first.removed.is_empty());

    let second = write_resources(&catalog, workspace).expect("连续生成应零写入");
    assert!(second.written.is_empty());
    assert!(second.removed.is_empty());
    assert_eq!(second.unchanged.len(), catalog.assets.len());

    let generated_file = backend
        .path()
        .join("crates/ryframe-application/src/generated/device/model.rs");
    fs::write(&generated_file, "// 人工修改\n").expect("应模拟人工修改");
    let error = write_resources(&catalog, workspace)
        .expect_err("人工修改必须安全失败")
        .to_string();
    assert!(error.contains("人工修改"));
    assert!(error.contains("model.rs"));

    let unaffected = frontend
        .path()
        .join("src/generated/resources/device/api.ts");
    assert!(
        fs::read_to_string(unaffected)
            .expect("前端文件应保留")
            .contains("export function listDevice")
    );
}

#[test]
fn writer_rejects_paths_outside_generated_boundaries_before_writing() {
    let backend = tempfile::tempdir().expect("应创建后端临时工作区");
    let frontend = tempfile::tempdir().expect("应创建前端临时工作区");
    fs::write(backend.path().join("Cargo.toml"), "[workspace]\n").expect("应创建工作区标识");
    let mut catalog = render_resources(&[device()]).expect("生成应成功");
    catalog.assets[0].root = AssetRoot::Backend;
    catalog.assets[0].path = "README.md".into();

    let error = write_resources(
        &catalog,
        ResourceWorkspace {
            backend_root: backend.path(),
            frontend_root: Some(frontend.path()),
        },
    )
    .expect_err("白名单外路径必须被拒绝")
    .to_string();
    assert!(error.contains("白名单"));
    assert!(!backend.path().join("README.md").exists());
}

#[test]
fn named_write_matches_preview_and_preserves_other_managed_resources() {
    let backend = tempfile::tempdir().expect("应创建后端临时工作区");
    let frontend = tempfile::tempdir().expect("应创建前端临时工作区");
    fs::write(backend.path().join("Cargo.toml"), "[workspace]\n").expect("应创建工作区标识");
    let catalog = render_resources(&[
        device(),
        load_resource(post_path()).expect("Post 资源清单应有效"),
    ])
    .expect("完整清单应生成");
    let workspace = ResourceWorkspace {
        backend_root: backend.path(),
        frontend_root: Some(frontend.path()),
    };

    write_resource(&catalog, "device", workspace).expect("首次写 Device 应成功");
    let device_file = backend
        .path()
        .join("crates/ryframe-application/src/generated/device/model.rs");
    let device_before = fs::read_to_string(&device_file).expect("Device 文件应存在");

    let preview = plan_resource_assets(&catalog, "post", workspace).expect("Post 预览应成功");
    let preview_paths = preview
        .iter()
        .map(|asset| format!("{}:{}", asset.root.label(), asset.path))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        preview
            .iter()
            .all(|asset| { asset.resource == "post" || asset.resource == "__catalog__" })
    );
    assert!(
        preview
            .iter()
            .all(|asset| { !asset.path.contains("/device/") })
    );

    let report = write_resource(&catalog, "post", workspace).expect("写 Post 应成功");
    let touched = report
        .written
        .iter()
        .chain(&report.unchanged)
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        preview_paths,
        touched.intersection(&preview_paths).cloned().collect()
    );
    assert!(report.removed.iter().all(|path| !path.contains("/device/")));
    assert!(report.written.iter().all(|path| !path.contains("/device/")));
    assert_eq!(
        fs::read_to_string(&device_file).expect("Post 写入后 Device 文件仍应存在"),
        device_before
    );

    let ownership = fs::read_to_string(backend.path().join("catalog/resources/.ownership.toml"))
        .expect("ownership 应存在");
    assert!(ownership.contains("resource = \"device\""));
    assert!(ownership.contains("resource = \"post\""));
}

#[test]
fn change_plan_reports_obsolete_assets_as_deletions() {
    let backend = tempfile::tempdir().expect("应创建后端临时工作区");
    let frontend = tempfile::tempdir().expect("应创建前端临时工作区");
    fs::write(backend.path().join("Cargo.toml"), "[workspace]\n").expect("应创建工作区标识");
    let post = load_resource(post_path()).expect("Post 资源清单应有效");
    let mut installed = render_resources(std::slice::from_ref(&post)).expect("Post 应生成");
    installed.assets.push(GeneratedAsset {
        resource: "post".into(),
        root: AssetRoot::Frontend,
        path: "src/generated/resources/post/obsolete.ts".into(),
        content: "// 已废弃生成资产\nexport const obsolete = true;\n".into(),
    });
    let workspace = ResourceWorkspace {
        backend_root: backend.path(),
        frontend_root: Some(frontend.path()),
    };
    write_resources(&installed, workspace).expect("旧版资产应先写入 ownership");

    let current = render_resources(&[post]).expect("当前 Post 应生成");
    let plan = plan_resource_changes(&current, "post", workspace).expect("预览应成功");
    let obsolete = plan
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("/obsolete.ts"))
        .expect("预览不能隐藏 ownership 中的删除");
    assert_eq!(obsolete.action, PlanAction::Delete);
    assert!(obsolete.before.contains("obsolete = true"));
    assert!(obsolete.after.is_empty());

    let report = write_resource(&current, "post", workspace).expect("写入应执行同一删除");
    assert!(
        report
            .removed
            .iter()
            .any(|path| path.ends_with("/obsolete.ts"))
    );
}

#[test]
fn missing_manifest_is_not_silently_treated_as_resource_removal() {
    let backend = tempfile::tempdir().expect("应创建后端临时工作区");
    let frontend = tempfile::tempdir().expect("应创建前端临时工作区");
    fs::write(backend.path().join("Cargo.toml"), "[workspace]\n").expect("应创建工作区标识");
    let post = load_resource(post_path()).expect("Post 资源清单应有效");
    let workspace = ResourceWorkspace {
        backend_root: backend.path(),
        frontend_root: Some(frontend.path()),
    };
    let current = render_resources(&[post]).expect("Post 应生成");
    write_resource(&current, "post", workspace).expect("首次写 Post 应成功");
    let empty = render_resources(&[]).expect("空聚合应可生成");

    let error = write_resource(&empty, "post", workspace)
        .expect_err("缺失清单不得静默遗留或删除旧资产")
        .to_string();

    assert!(error.contains("不能把缺失清单当作删除指令"));
    assert!(error.contains("当前版本不支持"));
    assert!(error.contains("显式 remove 命令"));

    let error = write_resources(&empty, workspace)
        .expect_err("完整空 catalog 也不得推断最后资源移除")
        .to_string();
    assert!(error.contains("拒绝把缺失清单推断为删除"));
    assert!(error.contains("当前版本尚不支持资源退役"));
}

#[test]
fn named_write_rejects_pending_changes_in_other_managed_resources() {
    let backend = tempfile::tempdir().expect("应创建后端临时工作区");
    let frontend = tempfile::tempdir().expect("应创建前端临时工作区");
    fs::write(backend.path().join("Cargo.toml"), "[workspace]\n").expect("应创建工作区标识");
    let post = load_resource(post_path()).expect("Post 资源清单应有效");
    let original_device = device();
    let initial = render_resources(&[original_device.clone(), post.clone()]).expect("清单应生成");
    let workspace = ResourceWorkspace {
        backend_root: backend.path(),
        frontend_root: Some(frontend.path()),
    };
    write_resource(&initial, "device", workspace).expect("首次写 Device 应成功");
    let device_file = backend
        .path()
        .join("crates/ryframe-application/src/generated/device/model.rs");
    let device_before = fs::read_to_string(&device_file).expect("Device 文件应存在");
    let ownership_path = backend.path().join("catalog/resources/.ownership.toml");
    let ownership_before = fs::read_to_string(&ownership_path).expect("ownership 应存在");

    let mut changed_device = original_device;
    changed_device.source_hash = "f".repeat(64);
    let changed_catalog = render_resources(&[changed_device, post]).expect("变更清单应生成预览");
    let error = write_resource(&changed_catalog, "post", workspace)
        .expect_err("不得让聚合入口提前采用未生成的 Device 清单")
        .to_string();

    assert!(error.contains("资源 device"));
    assert!(error.contains("待生成变化"));
    assert!(error.contains("cargo resource device --write"));
    assert_eq!(fs::read_to_string(device_file).unwrap(), device_before);
    assert_eq!(
        fs::read_to_string(ownership_path).unwrap(),
        ownership_before
    );
    assert!(
        !backend
            .path()
            .join("crates/ryframe-application/src/generated/post/model.rs")
            .exists()
    );
}

#[test]
fn control_initial_migration_has_tenant_fk_and_compact_enum_columns() {
    let source = fs::read_to_string(post_path())
        .expect("应读取 Post 清单")
        .replacen(
            "primary_key = [\"id\"]",
            "primary_key = [\"id\"]\nbootstrap_migration = true",
            1,
        );
    let spec = ResourceSpec::parse(&source, "catalog/resources/post.toml")
        .expect("control fixture TOML 应有效");
    let post = normalize_resource(spec, "catalog/resources/post.toml", "control-schema")
        .expect("control fixture 应通过 IR");
    let generated = render_resources(&[post]).expect("control fixture 应生成");
    let migration = generated
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("post/migration.rs"))
        .expect("应生成 control 初始迁移")
        .content
        .as_str();

    assert!(migration.contains("CONSTRAINT `fk_post_tenant`"));
    assert!(migration.contains("REFERENCES `sys_tenant` (`tenant_id`)"));
    assert!(migration.contains("`tenant_id` VARCHAR(64)"));
    assert!(migration.contains("`status` VARCHAR(1)"));
    let aggregate = generated
        .assets
        .iter()
        .find(|asset| asset.path == "crates/ryframe-db/src/generated/mod.rs")
        .expect("control generated 聚合应存在");
    assert!(
        aggregate
            .content
            .contains("Box::new(post::migration::Migration)")
    );
}

#[test]
fn initial_migration_is_immutable_and_schema_evolution_requires_new_revision() {
    let backend = tempfile::tempdir().expect("应创建后端临时工作区");
    let frontend = tempfile::tempdir().expect("应创建前端临时工作区");
    fs::write(backend.path().join("Cargo.toml"), "[workspace]\n").expect("应创建工作区标识");
    let workspace = ResourceWorkspace {
        backend_root: backend.path(),
        frontend_root: Some(frontend.path()),
    };
    let original = device();
    let rendered = render_resources(std::slice::from_ref(&original)).expect("Device 应生成");
    let aggregate = rendered
        .assets
        .iter()
        .find(|asset| asset.path == "crates/ryframe-tenant-db/src/generated/mod.rs")
        .expect("tenant generated 聚合应存在");
    assert!(
        aggregate
            .content
            .contains("Box::new(device::migration::Migration)")
    );
    write_resource(&rendered, "device", workspace).expect("首次写入应成功");
    let migration_path = backend
        .path()
        .join("crates/ryframe-tenant-db/src/generated/device/migration.rs");
    let initial_migration = fs::read_to_string(&migration_path).expect("初始迁移应存在");

    let mut label_only = original;
    label_only.source_hash = "a".repeat(64);
    label_only.labels.zh_cn = "终端设备".into();
    write_resource(
        &render_resources(&[label_only]).expect("标签变更应生成"),
        "device",
        workspace,
    )
    .expect("非 schema 变更应保留初始迁移");
    assert_eq!(
        fs::read_to_string(&migration_path).unwrap(),
        initial_migration
    );

    let unversioned_source = fs::read_to_string(device_path())
        .expect("应读取 Device fixture")
        .replacen("max_length = 100", "max_length = 110", 1);
    let unversioned = normalize_resource(
        ResourceSpec::parse(&unversioned_source, "catalog/resources/device.toml").unwrap(),
        "catalog/resources/device.toml",
        "schema-without-revision",
    )
    .expect("缺少 revision 不影响清单语法校验");
    let error = write_resource(
        &render_resources(&[unversioned]).expect("未声明 revision 的 schema 仍应可预览"),
        "device",
        workspace,
    )
    .expect_err("已受管 schema 变化必须声明新 revision")
    .to_string();
    assert!(error.contains("没有声明新的 schema_revision"));
    assert!(error.contains("cargo migrate new tenant-data"));

    let revision = "m20260823_123456_expand_device_name";
    let changed_source = fs::read_to_string(device_path())
        .expect("应读取 Device fixture")
        .replacen(
            "bootstrap_migration = true",
            &format!("bootstrap_migration = true\nschema_revision = {revision:?}"),
            1,
        )
        .replacen("max_length = 100", "max_length = 120", 1);
    let spec = ResourceSpec::parse(&changed_source, "catalog/resources/device.toml")
        .expect("schema 变更 TOML 应有效");
    let changed = normalize_resource(spec, "catalog/resources/device.toml", "schema-v2")
        .expect("schema v2 应通过 IR");
    let revision_path = backend
        .path()
        .join("crates/ryframe-tenant-db/src/migration")
        .join(format!("{revision}.rs"));
    fs::create_dir_all(revision_path.parent().unwrap()).expect("应创建追加迁移目录");
    fs::write(&revision_path, "// 待冻结的追加 roll-forward 迁移\n").expect("应写入追加迁移");
    write_resource(
        &render_resources(&[changed]).expect("schema v2 应生成"),
        "device",
        workspace,
    )
    .expect("新 revision 与未提交迁移应允许 schema 演进");
    assert_eq!(
        fs::read_to_string(&migration_path).unwrap(),
        initial_migration
    );
    let ownership = fs::read_to_string(backend.path().join("catalog/resources/.ownership.toml"))
        .expect("ownership 应存在");
    assert!(ownership.contains("schema_hash ="));
    assert!(ownership.contains(&format!("schema_revision = {revision:?}")));

    let reused_source = changed_source.replacen("max_length = 120", "max_length = 130", 1);
    let reused = normalize_resource(
        ResourceSpec::parse(&reused_source, "catalog/resources/device.toml").unwrap(),
        "catalog/resources/device.toml",
        "schema-v3",
    )
    .expect("schema v3 IR 本身应有效");
    let error = write_resource(
        &render_resources(&[reused]).expect("schema v3 应生成预览"),
        "device",
        workspace,
    )
    .expect_err("同 revision 不得承载第二次 schema 变化")
    .to_string();
    assert!(error.contains("已用于上一版 schema"));
    assert!(error.contains("cargo migrate new tenant-data"));

    let removed_source = changed_source.replacen(
        "bootstrap_migration = true",
        "bootstrap_migration = false",
        1,
    );
    let removed = normalize_resource(
        ResourceSpec::parse(&removed_source, "catalog/resources/device.toml").unwrap(),
        "catalog/resources/device.toml",
        "schema-v2-without-initial",
    )
    .expect("移除初始迁移标记仍是有效 IR");
    let error = write_resource(
        &render_resources(&[removed]).expect("无初始迁移资产的预览应可构造"),
        "device",
        workspace,
    )
    .expect_err("已落盘初始迁移不得被清单删除")
    .to_string();
    assert!(error.contains("初始迁移不可删除或改名"));
}
