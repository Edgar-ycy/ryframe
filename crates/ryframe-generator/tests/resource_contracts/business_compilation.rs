use std::{fs, path::Path, process::Command};

use ryframe_generator::business::{
    BusinessBootstrapOptions, BusinessGenerateOptions, bootstrap_business_package,
    generate_business_package,
};

pub(super) fn verify(backend: &Path, target: &Path) {
    bootstrap_business_package(BusinessBootstrapOptions {
        workspace_root: backend,
        module: "invoice",
        write: true,
    })
    .expect("真实 Workspace 应创建业务 crate 并装配进程");
    fs::write(
        backend.join("crates/invoice-business/src/resources/mod.rs"),
        MODEL,
    )
    .expect("应写入租户自增主键业务模型");
    let metadata = Command::new("cargo")
        .args(["metadata", "--offline", "--format-version", "1"])
        .current_dir(backend)
        .output()
        .unwrap();
    super::assert_command_succeeded("显式登记业务依赖锁文件", &metadata);
    let lock = fs::read(backend.join("Cargo.lock")).unwrap();
    let report = generate_business_package(BusinessGenerateOptions {
        current_dir: backend,
        package: "invoice-business",
        model: None,
        write: true,
    })
    .expect("应生成真实业务资源");
    assert!(!report.created.is_empty());
    let repeat = generate_business_package(BusinessGenerateOptions {
        current_dir: backend,
        package: "invoice-business",
        model: None,
        write: true,
    })
    .expect("业务资源重复生成应零差异");
    assert!(repeat.created.is_empty() && repeat.updated.is_empty() && repeat.removed.is_empty());
    assert_eq!(fs::read(backend.join("Cargo.lock")).unwrap(), lock);
    for features in ["catalog", "migration", "api"] {
        let output = Command::new("cargo")
            .args([
                "check",
                "--locked",
                "--offline",
                "--no-default-features",
                "-p",
                "invoice-business",
                "--features",
                features,
                "--jobs",
                "4",
            ])
            .current_dir(backend)
            .env("CARGO_TARGET_DIR", target)
            .output()
            .unwrap();
        super::assert_command_succeeded(&format!("生成业务 {features} 编译"), &output);
    }
    let formatted = Command::new("cargo")
        .args(["fmt", "--all", "--", "--check"])
        .current_dir(backend)
        .output()
        .unwrap();
    super::assert_command_succeeded("业务 crate 与装配代码格式", &formatted);
}

const MODEL: &str = r#"#[derive(ryframe_sdk::ResourceModel)]
#[resource(
    name = "invoice",
    title = "发票",
    table = "biz_invoice",
    database = "tenant"
)]
pub struct Invoice {
    #[resource(primary_key)]
    pub tenant_id: String,
    #[resource(primary_key, auto_increment)]
    pub id: i64,
    #[resource(unique, filter, sort)]
    pub code: String,
    #[resource(read_only)]
    pub created_at: ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>,
    #[resource(read_only)]
    pub updated_at: ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>,
}

pub fn descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> {
    vec![<Invoice as ryframe_sdk::ResourceModel>::descriptor()]
}
"#;
