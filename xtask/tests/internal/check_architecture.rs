use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use chrono::{Duration, Local};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::check::architecture::{
    self, ast, docs, policy, source_size, syntax, tenant, test_layout, workspace,
};

const DOCUMENTS: &[&str] = &[
    "README.md",
    "README.en.md",
    "CHANGELOG.md",
    "docs/api.md",
    "docs/architecture.md",
    "docs/data.md",
    "docs/development.md",
    "docs/operations.md",
];

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn test_dir() -> TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("architecture-")
        .tempdir_in(root)
        .unwrap()
}

fn documentation_fixture() -> TempDir {
    let directory = test_dir();
    for document in DOCUMENTS {
        write(directory.path(), document, "当前文档\n");
    }
    directory
}

fn documentation_errors(root: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    docs::validate_documentation(root, &mut errors);
    errors
}

fn system_errors(root: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    docs::validate_system_domain_surface(root, &mut errors);
    errors
}

fn tenant_errors(root: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    tenant::validate_tenant_data_boundaries(root, &mut errors);
    errors
}

fn package(
    root: &Path,
    name: &str,
    dependencies: Vec<PathBuf>,
    targets: Vec<workspace::Target>,
) -> workspace::Package {
    workspace::Package {
        name: name.to_owned(),
        id: format!("path+file:///{name}"),
        manifest_path: root.join(format!("crates/{name}/Cargo.toml")),
        dependencies: dependencies
            .into_iter()
            .map(|path| workspace::Dependency { path: Some(path) })
            .collect(),
        targets,
        metadata: None,
    }
}

fn table(source: &str) -> toml::Table {
    toml::from_str::<toml::Value>(source)
        .unwrap()
        .as_table()
        .cloned()
        .unwrap()
}

#[test]
fn documentation_gate_prunes_cache_directories_and_rejects_extra_documents() {
    let directory = documentation_fixture();
    write(directory.path(), ".pytest_cache/README.md", "工具缓存\n");
    assert!(documentation_errors(directory.path()).is_empty());

    write(directory.path(), "docs/notes.md", "额外说明\n");
    let errors = documentation_errors(directory.path());
    assert!(errors.iter().any(|error| error.contains("docs/notes.md")));
}

#[test]
fn documentation_gate_enforces_per_document_line_limits() {
    let directory = documentation_fixture();
    write(
        directory.path(),
        "README.md",
        &format!("{}\n", "正文\n".repeat(121)),
    );
    let errors = documentation_errors(directory.path());
    assert!(
        errors
            .iter()
            .any(|error| error.contains("README.md") && error.contains("120 行上限"))
    );
}

#[test]
fn system_surface_accepts_only_four_public_domains_and_no_root_reexports() {
    let directory = test_dir();
    write(
        directory.path(),
        "crates/ryframe-application/src/system/mod.rs",
        "pub mod content;\npub mod identity;\npub mod operations;\npub mod platform;\n",
    );
    assert!(system_errors(directory.path()).is_empty());

    write(
        directory.path(),
        "crates/ryframe-application/src/system/mod.rs",
        "pub mod content;\npub mod identity;\npub mod operations;\npub mod platform;\npub use identity::UserService;\n",
    );
    let errors = system_errors(directory.path());
    assert!(errors.iter().any(|error| error.contains("re-export")));
}

#[test]
fn source_size_thresholds_keep_eighty_ninety_and_hard_limit_semantics() {
    assert!(source_size::source_size_level(479, 600).is_none());
    assert!(matches!(
        source_size::source_size_level(480, 600),
        Some(source_size::SizeLevel::Hint)
    ));
    assert!(matches!(
        source_size::source_size_level(540, 600),
        Some(source_size::SizeLevel::Strong)
    ));
    assert!(matches!(
        source_size::source_size_level(600, 600),
        Some(source_size::SizeLevel::Fail)
    ));
}

#[test]
fn temporary_edge_policy_requires_reason_and_rejects_expired_registration() {
    let values = toml::from_str::<toml::Value>(
        "edges = [\n  { edge = \"first -> tool\" },\n  { edge = \"second -> tool\", reason = \"等待边界迁移\", expires = \"2000-01-01\" },\n]\n",
    )
    .unwrap();
    let mut errors = Vec::new();
    let edges = policy::parse_temporary_edges(values.get("edges"), "temporary", &mut errors);
    assert_eq!(
        edges,
        BTreeSet::from([("second".to_owned(), "tool".to_owned())])
    );
    assert!(errors.iter().any(|error| error.contains("缺少字段")));
    assert!(errors.iter().any(|error| error.contains("过期")));
}

#[test]
fn changed_ranges_ignore_deletion_hunks_and_expand_untracked_paths() {
    let diff = "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -2 +2,3 @@
+one
+two
+three
@@ -10,2 +12,0 @@
-old
-old
";
    let changed = ast::changed::parse_changed_ranges(diff);
    assert_eq!(changed.get("src/lib.rs"), Some(&vec![(2, 4)]));
    assert_eq!(
        ast::changed::parse_untracked_rust_paths("src\\new.rs\0README.md\0"),
        ["src/new.rs"]
    );
}

#[test]
fn frozen_migration_exclusion_requires_an_exact_sha256_match() {
    let directory = test_dir();
    let relative = "crates/example/src/migration/m20260820_baseline/schema.rs";
    write(
        directory.path(),
        relative,
        "pub const BASELINE: &str = \"stable\";\n",
    );
    let digest = Sha256::digest(fs::read(directory.path().join(relative)).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    write(
        directory.path(),
        "catalog/migrations.lock.toml",
        &format!("format_version = 1\n[[files]]\npath = \"{relative}\"\nsha256 = \"{digest}\"\n"),
    );
    let mut errors = Vec::new();
    let frozen = source_size::frozen_migration_sources(
        directory.path(),
        "catalog/migrations.lock.toml",
        &mut errors,
    );
    assert_eq!(frozen, BTreeSet::from([relative.to_owned()]));
    assert!(errors.is_empty());

    write(
        directory.path(),
        "catalog/migrations.lock.toml",
        &format!(
            "format_version = 1\n[[files]]\npath = \"{relative}\"\nsha256 = \"{}\"\n",
            "0".repeat(64)
        ),
    );
    let mut errors = Vec::new();
    let frozen = source_size::frozen_migration_sources(
        directory.path(),
        "catalog/migrations.lock.toml",
        &mut errors,
    );
    assert!(frozen.is_empty());
    assert!(errors.iter().any(|error| error.contains("哈希不匹配")));
}

#[test]
fn legacy_and_async_port_gates_reject_removed_interfaces() {
    let directory = test_dir();
    write(
        directory.path(),
        "crates/example/src/port.rs",
        &format!(
            "pub trait {}Future {{}}\npub trait Port {{ fn execute(&self) -> WorkFuture<'_>; }}\n",
            "Persistence"
        ),
    );
    let sources = ["crates/example/src/port.rs".to_owned()];
    let mut legacy = Vec::new();
    syntax::validate_legacy_persistence_apis(directory.path(), &sources, &mut legacy);
    let removed = format!("{}{}", "Persistence", "Future");
    assert!(legacy.iter().any(|error| error.contains(&removed)));
    let mut async_errors = Vec::new();
    syntax::validate_async_port_traits(directory.path(), &sources, &mut async_errors);
    assert!(
        async_errors
            .iter()
            .any(|error| error.contains("Port::execute"))
    );

    write(
        directory.path(),
        "crates/example/src/port.rs",
        "pub trait Port { async fn execute(&self) -> Result<(), String>; }\n",
    );
    let mut legacy = Vec::new();
    syntax::validate_legacy_persistence_apis(directory.path(), &sources, &mut legacy);
    let mut async_errors = Vec::new();
    syntax::validate_async_port_traits(directory.path(), &sources, &mut async_errors);
    assert!(legacy.is_empty());
    assert!(async_errors.is_empty());
}

#[test]
fn workspace_lint_and_active_edges_are_checked_against_exact_declarations() {
    let directory = test_dir();
    write(
        directory.path(),
        "Cargo.toml",
        "[workspace]\n[workspace.lints.rust]\nunsafe_code = \"warn\"\n",
    );
    write(
        directory.path(),
        "crates/first/Cargo.toml",
        "[package]\nname = \"first\"\nversion = \"0.1.0\"\n",
    );
    write(
        directory.path(),
        "crates/second/Cargo.toml",
        "[package]\nname = \"second\"\nversion = \"0.1.0\"\n",
    );
    let second_root = directory.path().join("crates/second");
    let packages = workspace::Packages::from([
        (
            "first".to_owned(),
            package(directory.path(), "first", vec![second_root], Vec::new()),
        ),
        (
            "second".to_owned(),
            package(directory.path(), "second", Vec::new(), Vec::new()),
        ),
    ]);
    let mut errors = Vec::new();
    workspace::validate_unsafe_lint_policy(directory.path(), &packages, &mut errors);
    assert!(errors.iter().any(|error| error.contains("unsafe_code")));
    assert!(errors.iter().any(|error| error.contains("继承")));

    let profile = policy::Profile {
        packages: BTreeSet::from(["first".to_owned(), "second".to_owned()]),
        products: BTreeSet::from(["first".to_owned(), "second".to_owned()]),
        tools: BTreeSet::new(),
        allowed_edges: BTreeSet::from([
            ("first".to_owned(), "second".to_owned()),
            ("second".to_owned(), "first".to_owned()),
        ]),
        temporary_edges: BTreeSet::new(),
        expected_count: 2,
    };
    let mut errors = Vec::new();
    workspace::validate_active_workspace(
        directory.path(),
        "final",
        &profile,
        &packages,
        &mut errors,
    );
    assert!(errors.iter().any(|error| error.contains("已不存在")));
}

#[test]
fn business_crates_are_excluded_from_core_count_and_keep_strict_edges() {
    let directory = test_dir();
    for name in ["first", "second", "order-business"] {
        write(
            directory.path(),
            &format!("crates/{name}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
        );
    }
    let second_root = directory.path().join("crates/second");
    let mut business = package(
        directory.path(),
        "order-business",
        vec![second_root],
        Vec::new(),
    );
    business.metadata = Some(serde_json::json!({
        "ryframe": { "kind": "business", "module": "order" }
    }));
    let packages = workspace::Packages::from([
        (
            "first".to_owned(),
            package(directory.path(), "first", Vec::new(), Vec::new()),
        ),
        (
            "second".to_owned(),
            package(directory.path(), "second", Vec::new(), Vec::new()),
        ),
        ("order-business".to_owned(), business),
    ]);
    let profile = policy::Profile {
        packages: BTreeSet::from(["first".to_owned(), "second".to_owned()]),
        products: BTreeSet::from(["first".to_owned(), "second".to_owned()]),
        tools: BTreeSet::new(),
        allowed_edges: BTreeSet::new(),
        temporary_edges: BTreeSet::new(),
        expected_count: 2,
    };
    let mut errors = Vec::new();
    workspace::validate_active_workspace(
        directory.path(),
        "final",
        &profile,
        &packages,
        &mut errors,
    );
    assert!(!errors.iter().any(|error| error.contains("核心包数漂移")));
    assert!(errors.iter().any(|error| error.contains("业务 crate 只能")));
}

#[test]
fn test_layout_keeps_empty_binary_harnesses_registered_and_clean() {
    let directory = test_dir();
    write(
        directory.path(),
        "crates/example/Cargo.toml",
        "[package]\nname = \"example\"\nversion = \"0.1.0\"\n",
    );
    write(
        directory.path(),
        "crates/example/src/main.rs",
        "fn main() {}\n\n#[cfg(test)]\nmod tests { #[test] fn checks() {} }\n",
    );
    let source = directory.path().join("crates/example/src/main.rs");
    let packages = workspace::Packages::from([(
        "example".to_owned(),
        package(
            directory.path(),
            "example",
            Vec::new(),
            vec![workspace::Target {
                name: Some("tool".into()),
                kind: vec!["bin".into()],
                src_path: source,
                test: Some(false),
            }],
        ),
    )]);
    let raw = table(
        "directory = \"tests\"\nallow_colocated_unit_tests = true\nforbid_source_test_files = true\nmax_integration_test_lines = 1000\ndisabled_bin_test_targets = []\n",
    );
    let mut errors = Vec::new();
    test_layout::validate_test_layout(directory.path(), &raw, &packages, &mut errors);
    assert!(errors.iter().any(|error| error.contains("必须登记")));

    let raw = table(
        "directory = \"tests\"\nallow_colocated_unit_tests = true\nforbid_source_test_files = true\nmax_integration_test_lines = 1000\ndisabled_bin_test_targets = [\"example:tool\"]\n",
    );
    let mut errors = Vec::new();
    test_layout::validate_test_layout(directory.path(), &raw, &packages, &mut errors);
    assert!(errors.iter().any(|error| error.contains("不得包含单测")));
}

#[test]
fn tenant_boundary_gate_covers_catalog_binding_and_controlled_session() {
    let directory = test_dir();
    write(
        directory.path(),
        "catalog/resources/order.toml",
        "[storage]\nkind = \"tenant_data\"\n[database]\ntable = \"biz_order\"\n",
    );
    write(
        directory.path(),
        "crates/ryframe-tenant-db/src/generated/catalog.rs",
        "TenantDataTableDescriptor { table: \"biz_order\" }\n",
    );
    write(
        directory.path(),
        "crates/ryframe-tenant-db/src/generated/mod.rs",
        "pub mod catalog;\n",
    );
    write(
        directory.path(),
        "crates/ryframe-tenant-db/src/migration/catalog.rs",
        "tables: Cow::Borrowed(crate::generated::catalog::GENERATED_TENANT_DATA_TABLES)\ntenant_data_schema_fingerprint()\nself.validate_structure()?\ncomputed != expected\nCow::Borrowed(_)\nsuper::business_catalog::runtime_fingerprint()\n",
    );
    write(
        directory.path(),
        "crates/ryframe-tenant-db/src/migration/business_catalog.rs",
        "OnceLock<RuntimeCatalog>\nTENANT_DATA_CATALOG.validate_structure()?\nGENERATED_TENANT_DATA_SCHEMA_FINGERPRINT\ncatalog.validate_structure()?\ncurrent.catalog != candidate.catalog\nforeign_key_dependencies\n",
    );
    write(
        directory.path(),
        "crates/ryframe-generator/src/resource/render/tenant_catalog.rs",
        "TenantDataTableDescriptor\nprimary_key_cursor_columns\nchecksum_columns\ncolumn_types\nschema_canonical\nschema::column_type(field)\n",
    );
    write(
        directory.path(),
        "crates/ryframe-generator/src/resource/render/slice/migration.rs",
        "super::super::schema::column_type(field)\n",
    );
    assert!(tenant_errors(directory.path()).is_empty());

    write(
        directory.path(),
        "crates/ryframe-tenant-db/src/generated/business/repository.rs",
        "router.open_target(\"arbitrary\");\n",
    );
    let errors = tenant_errors(directory.path());
    assert!(errors.iter().any(|error| error.contains("绕过受控会话")));

    write(
        directory.path(),
        "crates/ryframe-tenant-db/src/migration/business_catalog.rs",
        "OnceLock<RuntimeCatalog>\nTENANT_DATA_CATALOG.validate_structure()?\nGENERATED_TENANT_DATA_SCHEMA_FINGERPRINT\ncatalog.validate_structure()?\ncurrent.catalog != candidate.catalog\n",
    );
    let errors = tenant_errors(directory.path());
    assert!(
        errors
            .iter()
            .any(|error| error.contains("foreign_key_dependencies"))
    );
}

#[test]
fn rust_ast_bridge_preserves_tree_sitter_function_and_unsafe_records() {
    let directory = test_dir();
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    fs::create_dir_all(directory.path().join("tools/python")).unwrap();
    fs::copy(
        source_root.join("tools/python/rust_function_size.py"),
        directory.path().join("tools/python/rust_function_size.py"),
    )
    .unwrap();
    write(
        directory.path(),
        "crates/example/src/lib.rs",
        "pub trait Port { fn declaration(&self); }\nimpl Port for Service { pub fn execute(&self) { let text = \"fn fake() {}\"; } }\nfn helper() {}\n",
    );
    write(
        directory.path(),
        "crates/example/templates/unsafe.rs.tpl",
        "pub fn {name}() { unsafe { call(); } }\n",
    );
    let function_size = toml::from_str::<toml::Value>(
        "mode = \"all\"\npublic_max_lines = 150\nprivate_max_lines = 100\norchestration_max_lines = 80\nmax_exception_days = 30\norchestrations = []\nexceptions = []\n",
    )
    .unwrap()
    .as_table()
    .cloned()
    .unwrap();
    let mut errors = Vec::new();
    let stats = ast::validate_ast(
        directory.path(),
        &["crates/example/src/lib.rs".to_owned()],
        &["crates/example/templates/unsafe.rs.tpl".to_owned()],
        &function_size,
        &Default::default(),
        &mut errors,
    );
    assert_eq!(stats.checked_unsafe_sources, 2);
    assert_eq!(stats.unsafe_violations, 1);
    assert_eq!(stats.checked_functions, 2);
    assert_eq!(stats.function_violations, 0);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("unsafe"));
}

#[test]
fn ast_helper_process_failure_produces_an_explicit_diagnostic() {
    let directory = test_dir();
    write(
        directory.path(),
        "crates/example/src/lib.rs",
        "fn safe() {}\n",
    );
    let function_size = table(
        "mode = \"all\"\npublic_max_lines = 150\nprivate_max_lines = 100\norchestration_max_lines = 80\nmax_exception_days = 30\norchestrations = []\nexceptions = []\n",
    );
    let mut errors = Vec::new();
    let stats = ast::validate_ast(
        directory.path(),
        &["crates/example/src/lib.rs".to_owned()],
        &[],
        &function_size,
        &Default::default(),
        &mut errors,
    );
    assert_eq!(stats.checked_unsafe_sources, 1);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("Rust AST helper 执行失败"))
    );
}

#[test]
fn function_policy_applies_changed_ranges_orchestration_and_exception_growth() {
    let expires = (Local::now().date_naive() + Duration::days(10))
        .format("%Y-%m-%d")
        .to_string();
    let raw = table(&format!(
        "mode = \"changed\"\npublic_max_lines = 6\nprivate_max_lines = 4\norchestration_max_lines = 3\nmax_exception_days = 30\norchestrations = [{{ path = \"src/lib.rs\", symbol = \"run\" }}]\nexceptions = [{{ path = \"src/lib.rs\", symbol = \"legacy\", current_lines = 4, reason = \"拆分中\", expires = {expires} }}]\n"
    ));
    let mut errors = Vec::new();
    let parsed = ast::parse_policy(&raw, &mut errors).unwrap();
    let records = vec![
        ast::FunctionRecord {
            path: "src/lib.rs".into(),
            symbol: "run".into(),
            start_line: 1,
            end_line: 4,
            lines: 4,
            private: false,
        },
        ast::FunctionRecord {
            path: "src/lib.rs".into(),
            symbol: "old".into(),
            start_line: 10,
            end_line: 20,
            lines: 11,
            private: true,
        },
        ast::FunctionRecord {
            path: "src/lib.rs".into(),
            symbol: "legacy".into(),
            start_line: 30,
            end_line: 35,
            lines: 6,
            private: true,
        },
    ];
    let changed = BTreeMap::from([("src/lib.rs".to_owned(), vec![(3, 3)])]);
    let (checked, violations) =
        ast::validate_functions(&parsed, &records, Some(&changed), &mut errors);
    assert_eq!(checked, 1);
    assert_eq!(violations, 2);
    assert!(errors.iter().any(|error| error.contains("编排函数")));
    assert!(errors.iter().any(|error| error.contains("只能缩短")));
}

#[test]
fn checked_in_workspace_passes_the_rust_architecture_task() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    architecture::run(root).unwrap();
}

#[test]
fn failed_architecture_check_preserves_exit_code_and_writes_nothing() {
    let directory = test_dir();
    let before = fs::read_dir(directory.path()).unwrap().count();
    let error = architecture::run(directory.path()).unwrap_err();
    let after = fs::read_dir(directory.path()).unwrap().count();
    assert_eq!(super::process::failure_exit_code(error.as_ref()), Some(1));
    assert_eq!(before, after);
}
