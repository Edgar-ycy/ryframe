use std::{collections::BTreeSet, fs, path::Path};

use ryframe_generator::business::{BusinessGenerateOptions, generate_business_package};

#[test]
fn business_crate_generation_is_local_read_only_by_default_and_repeatable() {
    assert_business_generation(false);
}

#[test]
fn auto_increment_business_preserves_tenant_primary_key_and_repeatability() {
    assert_business_generation(true);
}

fn assert_business_generation(auto_increment: bool) {
    let framework = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("生成器应位于框架 crates 目录");
    let workspace = tempfile::Builder::new()
        .prefix("business-generation-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
        .expect("应创建隔离业务工作区");
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ryframe-sdk");
    fs::create_dir_all(workspace.path().join("order-business/src/resources"))
        .expect("应创建业务源码目录");
    write_fixture_workspace(workspace.path(), framework);
    write_business_manifest(workspace.path(), &sdk);
    fs::write(
        workspace.path().join("order-business/src/lib.rs"),
        r#"pub mod resources;

pub fn resource_descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> {
    vec![<resources::Order as ryframe_sdk::ResourceModel>::descriptor()]
}
"#,
    )
    .expect("应写入业务入口");
    write_order_model(workspace.path());
    let locked = prepare_lock(workspace.path(), framework);
    if auto_increment {
        let model = workspace.path().join("order-business/src/resources/mod.rs");
        fs::write(
            &model,
            fs::read_to_string(&model)
                .unwrap()
                .replace("primary_key, generated", "primary_key, auto_increment"),
        )
        .unwrap();
    }

    let crate_root = workspace.path().join("order-business/src/resources");
    let preview = generate_business_package(BusinessGenerateOptions {
        catalog_target: None,
        current_dir: &crate_root,
        package: "order-business",
        models: &["Order".into()],
        write: false,
    })
    .expect("预览应成功");
    assert!(!preview.created.is_empty());
    assert!(
        !workspace
            .path()
            .join("order-business/src/generated")
            .exists()
    );

    let written = generate_business_package(BusinessGenerateOptions {
        catalog_target: None,
        current_dir: &crate_root,
        package: "order-business",
        models: &[],
        write: true,
    })
    .expect("写入应成功");
    assert!(!written.created.is_empty());
    assert_generated_business(workspace.path());
    if auto_increment {
        assert_auto_increment_assets(workspace.path());
    }

    let ownership_path = workspace
        .path()
        .join("order-business/.ryframe/generated.toml");
    let ownership_modified = fs::metadata(&ownership_path).unwrap().modified().unwrap();
    let repeat = generate_business_package(BusinessGenerateOptions {
        catalog_target: None,
        current_dir: &crate_root,
        package: "order-business",
        models: &[],
        write: true,
    })
    .expect("重复写入应成功且零差异");
    assert!(repeat.created.is_empty());
    assert!(repeat.updated.is_empty());
    assert!(repeat.removed.is_empty());
    assert_eq!(
        fs::metadata(&ownership_path).unwrap().modified().unwrap(),
        ownership_modified
    );

    assert_migration_is_immutable(workspace.path(), &crate_root);
    assert_incremental_generation(workspace.path(), &crate_root);
    assert_eq!(
        fs::read(workspace.path().join("Cargo.lock")).unwrap(),
        locked
    );
}

fn write_fixture_workspace(workspace: &Path, framework: &Path) {
    let source: toml::Value = toml::from_str(
        &fs::read_to_string(framework.join("Cargo.toml")).expect("应读取当前框架清单"),
    )
    .expect("框架清单应为有效 TOML");
    let mut manifest: toml::Value =
        toml::from_str("[workspace]\nmembers = [\"order-business\"]\nresolver = \"3\"\n")
            .expect("隔离工作区清单应为有效 TOML");
    if let Some(mut patches) = source.get("patch").cloned() {
        for (_, entries) in patches.as_table_mut().expect("patch 应为来源表").iter_mut() {
            for (_, entry) in entries.as_table_mut().expect("patch 来源应为表").iter_mut() {
                if let Some(value) = entry.get("path") {
                    let path = Path::new(value.as_str().expect("patch 路径应为字符串"));
                    let absolute = if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        framework.join(path)
                    };
                    entry.as_table_mut().expect("patch 条目应为表").insert(
                        "path".into(),
                        toml::Value::String(absolute.to_string_lossy().into_owned()),
                    );
                }
            }
        }
        manifest
            .as_table_mut()
            .expect("工作区清单应为表")
            .insert("patch".into(), patches);
    }
    fs::write(
        workspace.join("Cargo.toml"),
        toml::to_string(&manifest).expect("应序列化隔离工作区清单"),
    )
    .expect("应写入工作区清单");
}

fn prepare_lock(workspace: &Path, framework: &Path) -> Vec<u8> {
    let error = generate_business_package(BusinessGenerateOptions {
        catalog_target: None,
        current_dir: workspace,
        package: "order-business",
        models: &[],
        write: false,
    })
    .expect_err("缺少项目锁文件必须在生成前拒绝");
    assert!(
        error
            .to_string()
            .contains("cargo generate-lockfile --offline")
    );
    assert!(!workspace.join("target/ryframe-generate").exists());
    let framework_lock = fs::read(framework.join("Cargo.lock")).expect("应读取当前框架锁文件");
    fs::write(workspace.join("Cargo.lock"), &framework_lock).expect("应复制框架锁到隔离工作区");
    // 沿用真实 SDK 与 vendor 的锁定来源，仅在夹具中登记业务包，不重新挑选依赖版本。
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--offline", "--format-version", "1"])
        .current_dir(workspace)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let locked = fs::read(workspace.join("Cargo.lock")).expect("应读取登记业务包后的隔离锁文件");
    assert_fixture_lock(workspace, &framework_lock, &locked);
    assert!(!workspace.join("target/ryframe-generate").exists());
    locked
}

type LockedPackage = (String, String, Option<String>, Option<String>);

fn lock_identities(bytes: &[u8]) -> BTreeSet<LockedPackage> {
    let lock: toml::Value = toml::from_str(std::str::from_utf8(bytes).expect("锁文件应为 UTF-8"))
        .expect("锁文件应为有效 TOML");
    lock.get("package")
        .and_then(toml::Value::as_array)
        .expect("锁文件应包含 package 数组")
        .iter()
        .map(|package| {
            let field = |name| {
                package
                    .get(name)
                    .map(|value| value.as_str().expect("锁定身份字段应为字符串").to_owned())
            };
            (
                field("name").expect("锁定包应有名称"),
                field("version").expect("锁定包应有版本"),
                field("source"),
                field("checksum"),
            )
        })
        .collect()
}

fn assert_fixture_lock(workspace: &Path, framework_lock: &[u8], fixture_lock: &[u8]) {
    let manifest: toml::Value = toml::from_str(
        &fs::read_to_string(workspace.join("order-business/Cargo.toml"))
            .expect("应读取夹具业务清单"),
    )
    .expect("夹具业务清单应为有效 TOML");
    let business = (
        "order-business".to_owned(),
        manifest["package"]["version"]
            .as_str()
            .expect("夹具业务包应声明版本")
            .to_owned(),
        None,
        None,
    );
    let framework = lock_identities(framework_lock);
    let fixture = lock_identities(fixture_lock);
    assert!(!framework.contains(&business), "业务夹具不应属于框架锁");
    assert_eq!(
        fixture.difference(&framework).collect::<Vec<_>>(),
        vec![&business],
        "隔离锁只能新增本地业务包，其余包的名称、版本、来源和校验和必须来自当前框架锁"
    );
}

fn assert_auto_increment_assets(workspace: &std::path::Path) {
    let migration =
        fs::read_to_string(workspace.join("order-business/migrations/m_resource_initial_order.rs"))
            .unwrap();
    assert!(migration.contains("AUTO_INCREMENT"));
    assert!(migration.contains("PRIMARY KEY (`tenant_id`, `order_id`)"));
    assert!(migration.contains("UNIQUE KEY `uk_external_code` (`tenant_id`, `code`)"));
    assert!(!migration.contains("uk_order_code"));
    assert!(!migration.contains("idx_order_tenant_id"));
    assert!(migration.contains("KEY `idx_order_id` (`order_id`)"));
    let repository =
        fs::read_to_string(workspace.join("order-business/src/generated/repositories/order.rs"))
            .unwrap();
    assert!(repository.contains("LAST_INSERT_ID()"));
}

fn write_business_manifest(workspace: &std::path::Path, sdk: &std::path::Path) {
    fs::write(
        workspace.join("order-business/Cargo.toml"),
        format!(
            "[package]\nname = \"order-business\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[package.metadata.ryframe]\nkind = \"business\"\nmodule = \"sales-orders\"\n\n[features]\ndefault = []\ncatalog = []\n\n[dependencies]\nryframe-sdk = {{ path = {:?} }}\n",
            sdk.to_string_lossy()
        ),
    )
    .expect("应写入业务清单");
}

fn write_order_model(workspace: &std::path::Path) {
    fs::write(
        workspace.join("order-business/src/resources/mod.rs"),
        r#"#[derive(ryframe_sdk::ResourceModel)]
#[resource(name = "order", title = "订单", table = "biz_order", database = "tenant", primary_key = ["tenant_id", "id"])]
#[resource(index(name="uk_external_code",fields=["tenant_id","code"],unique))]
pub struct Order {
    #[resource(primary_key, generated, column = "order_id")]
    pub id: i64,
    #[resource(primary_key)]
    pub tenant_id: String,
    #[resource(unique, filter, sort)]
    pub code: String,
    #[resource(read_only)]
    pub created_at: ryframe_sdk::time::OffsetDateTime,
    #[resource(read_only)]
    pub updated_at: ryframe_sdk::time::OffsetDateTime,
}
"#,
    )
    .expect("应写入资源模型");
}

fn assert_generated_business(workspace: &std::path::Path) {
    let access =
        fs::read_to_string(workspace.join("order-business/src/generated/access.toml")).unwrap();
    let access: toml::Value = toml::from_str(&access).unwrap();
    assert_eq!(
        access["resources"][0]["menu"]["key"].as_str(),
        Some("sales-orders.order")
    );
    assert_eq!(access["resources"][0]["module"].as_str(), Some("business"));
    assert!(
        fs::read_to_string(workspace.join("order-business/src/generated/access.rs"))
            .unwrap()
            .contains("include_str!(\"access.toml\")")
    );
    let entity =
        fs::read_to_string(workspace.join("order-business/src/generated/entities/order.rs"))
            .unwrap();
    assert!(entity.find("pub tenant_id:").unwrap() < entity.find("pub id:").unwrap());
    assert!(entity.contains("column_name = \"order_id\""));
    let migration =
        fs::read_to_string(workspace.join("order-business/migrations/m_resource_initial_order.rs"))
            .unwrap();
    assert!(
        migration.find("`order_id` BIGINT").unwrap()
            < migration.find("`tenant_id` VARCHAR").unwrap()
    );
    assert!(migration.contains("PRIMARY KEY (`tenant_id`, `order_id`)"));
    let router = fs::read_to_string(workspace.join("order-business/src/generated/mod.rs")).unwrap();
    let openapi =
        fs::read_to_string(workspace.join("order-business/src/generated/openapi/order.rs"))
            .unwrap();
    assert!(router.contains("\"/sales_orders/order\""));
    assert!(openapi.contains("/api/v1/business/sales_orders/order"));
    assert!(openapi.contains("sales-orders.order"));
    assert!(!openapi.contains("sales_orders.order"));
    let field = fs::read_to_string(
        workspace.join("order-business/src/generated/openapi/order/fields/field_code.rs"),
    )
    .unwrap();
    assert!(field.contains("\"name\": \"code\""));
    assert!(!router.contains("/sales-orders/order"));
    let repository =
        fs::read_to_string(workspace.join("order-business/src/generated/repositories/order.rs"))
            .expect("应读取业务 Repository");
    assert!(
        !repository.contains("ActiveValue::Set"),
        "物理删除资源不应生成未使用的软删导入"
    );
    assert!(
        workspace
            .join("order-business/src/generated/entities/order.rs")
            .is_file()
    );
    assert!(
        workspace
            .join("order-business/migrations/m_resource_initial_order.rs")
            .is_file()
    );
    assert!(
        fs::read_to_string(workspace.join("order-business/src/generated/mod.rs"))
            .expect("应读取聚合模块")
            .contains("x-ryframe-crud-resources")
    );
    assert!(
        fs::read_to_string(workspace.join("order-business/src/generated/migrations/mod.rs"))
            .unwrap()
            .contains("with_initialization_sql")
    );
    let migration =
        fs::read(workspace.join("order-business/migrations/m_resource_initial_order.rs")).unwrap();
    let checksum = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(migration));
    let registry =
        fs::read_to_string(workspace.join("order-business/src/generated/migrations/mod.rs"))
            .unwrap();
    assert!(
        registry.contains(&checksum),
        "迁移注册必须使用最终格式化文件的校验和"
    );
    assert!(
        !workspace
            .join("order-business/src/resources/order.rs")
            .exists()
    );
}

fn assert_migration_is_immutable(workspace: &std::path::Path, crate_root: &std::path::Path) {
    let model_path = workspace.join("order-business/src/resources/mod.rs");
    let original = fs::read_to_string(&model_path).expect("应读取资源模型");
    let changed = original.replace(
        "    pub code: String,",
        "    pub code: String,\n    pub remark: String,",
    );
    fs::write(&model_path, changed).expect("应更新资源模型");
    let error = generate_business_package(BusinessGenerateOptions {
        catalog_target: None,
        current_dir: crate_root,
        package: "order-business",
        models: &[],
        write: false,
    })
    .expect_err("已生成的初始迁移不得随模型变化被覆盖");
    assert!(
        error.to_string().contains("人工审阅") && error.to_string().contains("回填"),
        "实际错误：{error}"
    );
    fs::write(model_path, original).unwrap();
}

fn assert_incremental_generation(workspace: &std::path::Path, crate_root: &std::path::Path) {
    let root = workspace.join("order-business");
    let model = root.join("src/resources/mod.rs");
    let initial = root.join("migrations/m_resource_initial_order.rs");
    let initial_bytes = fs::read(&initial).unwrap();
    let original = fs::read_to_string(&model).unwrap();
    fs::write(&model, original.replace("    pub code: String,", "    pub code: String,\n    pub remark: Option<String>,\n    #[resource(default = \"0\")]\n    pub amount: i32,")).unwrap();
    let preview = generate_business_package(BusinessGenerateOptions {
        catalog_target: None,
        current_dir: crate_root,
        package: "order-business",
        models: &[],
        write: false,
    })
    .unwrap();
    let first = preview
        .created
        .iter()
        .find(|path| path.starts_with("migrations/"))
        .unwrap()
        .clone();
    assert!(first.contains("_r000001_"));
    assert!(!root.join(&first).exists());
    assert!(
        !preview
            .updated
            .iter()
            .any(|path| path.starts_with("migrations/"))
    );
    let written = generate_business_package(BusinessGenerateOptions {
        catalog_target: None,
        current_dir: crate_root,
        package: "order-business",
        models: &[],
        write: true,
    })
    .unwrap();
    assert_eq!(written.created, preview.created);
    assert_eq!(fs::read(&initial).unwrap(), initial_bytes);
    let first_bytes = fs::read(root.join(&first)).unwrap();
    let source = String::from_utf8(first_bytes.clone()).unwrap();
    assert!(source.contains("ADD COLUMN `remark` VARCHAR(255) NULL AFTER `code`"));
    assert!(source.contains("ADD COLUMN `amount` INT NOT NULL DEFAULT 0 AFTER `remark`"));
    fs::write(
        &model,
        fs::read_to_string(&model)
            .unwrap()
            .replace("pub amount: i32", "pub amount: i64"),
    )
    .unwrap();
    let second = generate_business_package(BusinessGenerateOptions {
        catalog_target: None,
        current_dir: crate_root,
        package: "order-business",
        models: &[],
        write: true,
    })
    .unwrap();
    let second = second
        .created
        .iter()
        .find(|path| path.starts_with("migrations/"))
        .unwrap();
    assert!(second.contains("_r000002_"));
    assert!(
        fs::read_to_string(root.join(second))
            .unwrap()
            .contains("MODIFY COLUMN `amount` BIGINT NOT NULL DEFAULT 0")
    );
    assert_eq!(fs::read(&initial).unwrap(), initial_bytes);
    assert_eq!(fs::read(root.join(first)).unwrap(), first_bytes);
    let registry = fs::read_to_string(root.join("src/generated/migrations/mod.rs")).unwrap();
    assert_eq!(registry.matches("with_initialization_sql").count(), 3);
    assert_eq!(registry.matches("before_fingerprint: Some(").count(), 2);
    let repeat = generate_business_package(BusinessGenerateOptions {
        catalog_target: None,
        current_dir: crate_root,
        package: "order-business",
        models: &[],
        write: true,
    })
    .unwrap();
    assert!(repeat.created.is_empty() && repeat.updated.is_empty() && repeat.removed.is_empty());
    assert_drop_requires_review(crate_root, &root, &model);
}

fn assert_drop_requires_review(
    crate_root: &std::path::Path,
    root: &std::path::Path,
    model: &std::path::Path,
) {
    let ownership = fs::read(root.join(".ryframe/generated.toml")).unwrap();
    fs::write(
        model,
        fs::read_to_string(model)
            .unwrap()
            .replace("    pub remark: Option<String>,\n", ""),
    )
    .unwrap();
    let removed = generate_business_package(BusinessGenerateOptions {
        catalog_target: None,
        current_dir: crate_root,
        package: "order-business",
        models: &[],
        write: true,
    })
    .unwrap_err()
    .to_string();
    assert!(
        removed.contains("人工审阅")
            && removed.contains("remark")
            && removed.contains("删除或重命名")
    );
    assert_eq!(
        fs::read(root.join(".ryframe/generated.toml")).unwrap(),
        ownership
    );
}
