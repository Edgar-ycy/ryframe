use std::{fs, path::Path, process::Command};

use ryframe_generator::{
    business::{BusinessBootstrapOptions, bootstrap_business_package},
    command::{GenerationBuilder, GenerationOperation},
};

#[path = "../../src/command/staging/snapshot.rs"]
mod snapshot;

pub(super) fn verify() {
    let backend_source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let frontend_source = std::env::var_os("RYFRAME_RESOURCE_WORKSPACE_FRONTEND_DIR")
        .map(std::path::PathBuf::from)
        .expect("需要显式前端目录");
    let backend_before = snapshot::SourceSnapshot::capture(&backend_source).unwrap();
    let frontend_before = snapshot::SourceSnapshot::capture(&frontend_source).unwrap();
    assert!(
        backend_before
            .input_paths()
            .any(|path| path == Path::new("Cargo.toml"))
    );
    let backend_cache = backend_source.join(".local-tests/generation-pipeline");
    let frontend_cache = frontend_source.join(".local-tests/generation-pipeline");
    fs::create_dir_all(&backend_cache).unwrap();
    fs::create_dir_all(&frontend_cache).unwrap();
    let backend_scope = tempfile::tempdir_in(backend_cache).unwrap();
    let frontend_scope = tempfile::tempdir_in(frontend_cache).unwrap();
    let backend = backend_scope.path().join("backend");
    let frontend = frontend_scope.path().join("frontend");
    backend_before.copy_to(&backend).unwrap();
    frontend_before.copy_to(&frontend).unwrap();
    bootstrap_business_package(BusinessBootstrapOptions {
        workspace_root: &backend,
        module: "pipeline",
        write: true,
    })
    .unwrap();
    let model = backend.join("crates/pipeline-business/src/resources/mod.rs");
    fs::write(&model, MODELS).unwrap();
    let metadata = Command::new("cargo")
        .args(["metadata", "--offline", "--format-version", "1"])
        .current_dir(&backend)
        .output()
        .unwrap();
    assert!(
        metadata.status.success(),
        "{}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    fs::create_dir_all(backend.join(".local-tests")).unwrap();
    fs::write(backend.join(".local-tests/tools.toml"), format!("[workspace]\nfrontend_dir={:?}\n[generator]\npackage='pipeline-business'\nsync_frontend=true\n", frontend.to_string_lossy())).unwrap();
    let before = snapshot::SourceSnapshot::capture(&backend).unwrap();
    let before_front = snapshot::SourceSnapshot::capture(&frontend).unwrap();
    generate(&backend, false).unwrap();
    before.verify().unwrap();
    before_front.verify().unwrap();
    generate(&backend, true).unwrap();
    verify_outputs(&backend, &frontend);
    let changes = before.changes(&backend).unwrap();
    assert!(changes.iter().any(
        |change| change.relative == Path::new("openapi/openapi.json")
            && change.before.is_some()
            && change.after.is_some()
    ));
    let installed = snapshot::SourceSnapshot::capture(&backend).unwrap();
    let installed_front = snapshot::SourceSnapshot::capture(&frontend).unwrap();
    generate(&backend, true).unwrap();
    installed.verify().unwrap();
    installed_front.verify().unwrap();
    fs::write(
        &model,
        MODELS.replace(
            "pub name: String,",
            "pub name: String,\n    pub note: Option<String>,",
        ),
    )
    .unwrap();
    let fault = frontend.join("src/generation_pipeline_failure.ts");
    fs::write(&fault, "export const generationFailure: string = 1\n").unwrap();
    let failed_backend = snapshot::SourceSnapshot::capture(&backend).unwrap();
    let failed_frontend = snapshot::SourceSnapshot::capture(&frontend).unwrap();
    assert!(generate(&backend, true).unwrap_err().contains("本批未安装"));
    failed_backend.verify().unwrap();
    failed_frontend.verify().unwrap();
    fs::remove_file(fault).unwrap();
    backend_before.verify().unwrap();
    frontend_before.verify().unwrap();
}

fn generate(root: &Path, write: bool) -> Result<(), String> {
    GenerationBuilder::new(root)
        .operation(GenerationOperation::Resource)
        .package("pipeline-business")
        .sync_frontend(true)
        .write(write)
        .plan()?
        .execute()
}

fn verify_outputs(backend: &Path, frontend: &Path) {
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(backend.join("openapi/openapi.json")).unwrap()).unwrap();
    for resource in ["table1", "table2"] {
        let path = format!("/api/v1/business/pipeline/{resource}");
        assert!(
            document["paths"].get(&path).is_some(),
            "缺少生成接口 {path}"
        );
        let directory = frontend.join("src/generated/resources").join(resource);
        assert!(directory.join("page.vue").is_file());
        let registration = fs::read_to_string(directory.join("registration.ts")).unwrap();
        assert!(registration.contains(&format!("pipeline.{resource}")));
        assert!(registration.contains("requiresSystemTenant: true"));
        assert!(registration.contains("import(\"./page.vue\")"));
        let api = fs::read_to_string(directory.join("api.ts")).unwrap();
        assert!(api.contains("@/api/generated/operations/system"));
        let system =
            fs::read_to_string(frontend.join("src/api/generated/operations/system.ts")).unwrap();
        let core =
            fs::read_to_string(frontend.join("src/api/generated/operations/core.ts")).unwrap();
        assert!(system.contains(&format!("get_business_pipeline_{resource}")));
        assert!(!core.contains(&format!("get_business_pipeline_{resource}")));
    }
    assert_eq!(
        fs::read(backend.join("openapi/openapi.json")).unwrap(),
        fs::read(frontend.join("openapi/openapi.json")).unwrap()
    );
}

const MODELS: &str = r#"
#[derive(ryframe_sdk::ResourceModel)]
#[resource(name="table1",title="表一",table="table1",database="control",menu_parent="system")]
pub struct Table1 {
    #[resource(primary_key,auto_increment)] pub id: i64,
    pub name: String,
    #[resource(sql_type="DECIMAL(20,6)",default="0.125000")] pub price: ryframe_sdk::rust_decimal::Decimal,
}

#[derive(ryframe_sdk::ResourceModel)]
#[resource(name="table2",title="表二",table="table2",database="control",menu_parent="system")]
pub struct Table2 {
    #[resource(primary_key,auto_increment)] pub id: u64,
    pub name: String,
    pub reference_id: Option<i64>,
}

pub fn descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> {
    vec![<Table1 as ryframe_sdk::ResourceModel>::descriptor(), <Table2 as ryframe_sdk::ResourceModel>::descriptor()]
}
"#;
