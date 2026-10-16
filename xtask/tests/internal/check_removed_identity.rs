use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    check::{
        CheckExecutionState, CheckPlanMode, TaskExecutionMode, TaskExecutor,
        VerifyExecutionContext, VerifySelection, execute_registered_task, removed_identity::check,
    },
    process::failure_exit_code,
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

const CLEAN_FILES: &[(&str, &str)] = &[
    ("Cargo.toml", "[workspace]\n"),
    ("crates/api/Cargo.toml", "[dependencies]\n"),
    ("crates/api/src/lib.rs", "pub fn users() {}\n"),
    (
        "crates/api/src/response.rs",
        "pub struct StandardEnvelope;\n",
    ),
    (
        "crates/api/build_support/catalog.rs",
        "pub const TAGS: &[&str] = &[];\n",
    ),
    ("openapi/openapi.json", "{}"),
    (
        "sql/ryframe_config.sql",
        "CREATE TABLE sys_product (id BIGINT);",
    ),
    ("config/default.toml", "[auth]\n"),
    ("catalog/access.toml", "permissions = [\"user:list\"]"),
    ("前端 包含空格/package.json", "{}"),
    ("前端 包含空格/openapi/openapi.json", "{}"),
    (
        "前端 包含空格/src/index.ts",
        "export const capabilityCatalog = []",
    ),
    (
        "前端 包含空格/scripts/generate.mjs",
        "export const domains = ['core']",
    ),
    (
        "前端 包含空格/scripts/headers.ts",
        "export const headers = ['Accept']",
    ),
    ("前端 包含空格/vite.config.ts", "export default {}"),
    ("CHANGELOG.md", "service_accounts 历史功能"),
    ("AGENTS.md", "ServiceAccount 未来可能重新设计"),
    ("scripts/tests/removed.txt", "sys_service_account 删除断言"),
];

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        fs::create_dir_all(&parent).unwrap();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = parent.join(format!(
            "removed-identity-{}-{id}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let fixture = Self { root };
        for (path, content) in CLEAN_FILES {
            fixture.write(path, content);
        }
        fixture
    }

    fn path(&self, relative: &str) -> PathBuf {
        relative
            .split('/')
            .fold(self.root.clone(), |path, part| path.join(part))
    }

    fn frontend(&self) -> PathBuf {
        self.root.join("前端 包含空格")
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn errors(&self) -> Vec<String> {
        check(&self.root, Some(&self.frontend())).unwrap()
    }

    fn context(&self) -> VerifyExecutionContext {
        let mut context = VerifyExecutionContext::new_ci(&self.frontend()).unwrap();
        context.root = self.root.clone();
        context
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        assert_eq!(self.root.parent(), Some(parent.as_path()));
        assert!(
            !fs::symlink_metadata(&self.root)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn clean_product_excludes_history_future_notes_and_tool_assertions() {
    let fixture = Fixture::new();
    for path in [
        "docs/architecture.md",
        "xtask/tests/removed.rs",
        "tools/python/test_removed.py",
        "crates/api/tests/removed.rs",
        "前端 包含空格/CHANGELOG.md",
        "前端 包含空格/scripts/tests/removed.ts",
        "前端 包含空格/scripts/nested/tests/removed.ts",
        "前端 包含空格/tests/removed.ts",
    ] {
        fixture.write(path, "ServiceAccount Agent 查询");
    }
    assert!(fixture.errors().is_empty());
    assert!(check(&fixture.root, None).unwrap().is_empty());
}

#[test]
fn product_definitions_cover_every_original_location_and_extra_entry_points() {
    let fixture = Fixture::new();
    for (relative, content) in [
        ("crates/api/Cargo.toml", "service-accounts = \"1\""),
        ("crates/api/src/lib.rs", "pub struct ServiceDelegation;"),
        (
            "crates/api/src/response.rs",
            "pub struct PrebuiltApiEnvelope; // Agent 查询",
        ),
        (
            "crates/api/build_support/catalog.rs",
            "const TAG: &str = \"服务账号\";",
        ),
        ("config/default.toml", "[service_accounts]"),
        (
            "sql/ryframe_config.sql",
            "CREATE TABLE sys_service_credential (id BIGINT);",
        ),
        ("openapi/openapi.json", "{\"/api/v1/agent/v1/query\": {}}"),
        ("catalog/access.toml", "key = \"service-accounts\""),
        (
            "前端 包含空格/src/index.ts",
            "export { ServiceAccessAudit } from './audit'",
        ),
        (
            "前端 包含空格/scripts/generate.mjs",
            "export const key = 'RyFrameApiKey'",
        ),
        (
            "前端 包含空格/scripts/headers.ts",
            "export const key = 'X-RyFrame-Delegation'",
        ),
        ("Cargo.toml", "service_credentials"),
        ("crates/api/build.rs", "PepperKeyring"),
        ("deploy/service.yaml", "service_access_audits"),
        ("前端 包含空格/package.json", "service-delegations"),
        ("前端 包含空格/openapi/openapi.json", "AgentQuery"),
        ("前端 包含空格/vite.config.ts", "Agent API"),
        ("前端 包含空格/playwright.smoke.ts", "AgentUserResponse"),
        ("前端 包含空格/src/tests/assertion.ts", "ServiceAccount"),
    ] {
        let path = fixture.path(relative);
        let previous = fs::read(&path).ok();
        fixture.write(relative, content);
        let errors = fixture.errors();
        assert_eq!(
            errors,
            [format!("已移除的产品定义仍存在：{}:1", path.display())],
            "{relative}"
        );
        match previous {
            Some(bytes) => fs::write(path, bytes).unwrap(),
            None => fs::remove_file(path).unwrap(),
        }
    }
    assert!(fixture.errors().is_empty());
}

#[test]
fn symbols_preserve_case_sensitive_substring_and_separator_rules() {
    let fixture = Fixture::new();
    let mut symbols = [
        "ServiceAccount",
        "ServiceDelegation",
        "ServiceCredential",
        "ServiceAccessAudit",
        "PepperKeyring",
        "AgentQuery",
        "AgentPageQuery",
        "AgentCapabilityResponse",
        "AgentUserResponse",
        "AgentDepartmentResponse",
        "AgentPostResponse",
        "AgentDictionaryItemResponse",
        "AgentDictionaryResponse",
        "PrebuiltApiEnvelope",
        "RyFrameApiKey",
        "X-RyFrame-Delegation",
        "/api/v1/agent/",
        "Agent API",
        "Agent 查询",
        "服务账号",
        "服务委托",
        "服务访问审计",
        "个人服务委托",
        "服务身份",
    ]
    .map(str::to_owned)
    .to_vec();
    for separator in ["_", "-"] {
        for kind in ["account", "delegation", "credential"] {
            for plural in ["", "s"] {
                symbols.push(format!("service{separator}{kind}{plural}"));
            }
        }
        for inner in ["_", "-"] {
            for plural in ["", "s"] {
                symbols.push(format!("service{separator}access{inner}audit{plural}"));
            }
        }
        for kind in ["query", "capabilit", "capability", "directory", "limiter"] {
            symbols.push(format!("agent{separator}{kind}"));
        }
    }
    for symbol in symbols {
        fixture.write("crates/api/src/lib.rs", &format!("prefix{symbol}suffix"));
        assert_eq!(fixture.errors().len(), 1, "{symbol}");
    }
    fixture.write("crates/api/src/lib.rs", "serviceAccount SERVICE_ACCOUNT Agent Query agent_post AgentResponse agent/query service_accessaudit");
    assert!(fixture.errors().is_empty());
}

#[test]
fn empty_removed_paths_and_unsupported_suffix_names_still_fail() {
    let fixture = Fixture::new();
    for relative in [
        "crates/ryframe-application/src/agent/mod.rs",
        "crates/ryframe-application/src/ports/service_accounts/mod.rs",
        "crates/ryframe-application/src/system/service_account/mod.rs",
        "crates/ryframe-application/src/service_identity_secret.rs",
        "crates/ryframe-api/src/dto/agent_dto.rs",
        "crates/ryframe-api/src/handlers/agent_handler.rs",
        "crates/ryframe-db/src/application_ports/agent/mod.rs",
        "crates/ryframe-db/src/application_ports/service_accounts/mod.rs",
        "crates/ryframe-db/src/repositories/service_authorization_repo.rs",
        "前端 包含空格/src/api/generated/operations/agent.ts",
        "前端 包含空格/src/api/generated/schema/agent.ts",
        "前端 包含空格/src/features/service-accounts/manifest.ts",
        "前端 包含空格/src/views/system/service-accounts/index.vue",
        "config/ServiceAccount.txt",
    ] {
        fixture.write(relative, "");
        assert_eq!(
            fixture.errors(),
            [format!(
                "已移除的产品文件仍存在：{}",
                fixture.path(relative).display()
            )]
        );
        fs::remove_file(fixture.path(relative)).unwrap();
    }
    assert!(fixture.errors().is_empty());
}

#[test]
fn suffix_environment_bom_and_line_numbers_match_the_original_gate() {
    let fixture = Fixture::new();
    for suffix in [
        "rs", "toml", "json", "sql", "yml", "yaml", "js", "cjs", "mjs", "ts", "cts", "mts", "vue",
    ] {
        let relative = format!("config/sample.{suffix}");
        fixture.write(&relative, "\u{feff}ok\r\nServiceAccount\nAgentQuery\n");
        let path = fixture.path(&relative);
        assert_eq!(
            fixture.errors(),
            [
                format!("已移除的产品定义仍存在：{}:2", path.display()),
                format!("已移除的产品定义仍存在：{}:3", path.display()),
            ]
        );
        fs::remove_file(path).unwrap();
    }
    fixture.write("config/.env.local", "ServiceAccount");
    assert_eq!(fixture.errors().len(), 1);
    fs::remove_file(fixture.root.join("config/.env.local")).unwrap();
    fixture.write("config/notes.txt", "ServiceAccount");
    fixture.write("config/notes.RS", "ServiceAccount");
    assert!(fixture.errors().is_empty());
    fixture.write(
        "config/newlines.rs",
        "ok\rServiceAccount\u{85}AgentQuery\u{2028}PepperKeyring",
    );
    let errors = fixture.errors();
    assert_eq!(errors.len(), 3);
    for (error, number) in errors.iter().zip(2..=4) {
        assert!(error.ends_with(&format!(":{number}")), "{error}");
    }
}

#[test]
fn diagnostics_keep_file_and_line_errors_in_stable_path_order() {
    let fixture = Fixture::new();
    fixture.write("config/z.rs", "AgentQuery\nServiceAccount");
    fixture.write("config/a_ServiceAccount.rs", "ServiceDelegation");
    let errors = fixture.errors();
    let first = fixture.path("config/a_ServiceAccount.rs");
    let last = fixture.path("config/z.rs");
    assert_eq!(
        errors,
        [
            format!("已移除的产品文件仍存在：{}", first.display()),
            format!("已移除的产品定义仍存在：{}:1", first.display()),
            format!("已移除的产品定义仍存在：{}:1", last.display()),
            format!("已移除的产品定义仍存在：{}:2", last.display()),
        ]
    );
    assert_eq!(fixture.errors(), errors);
}

#[test]
fn relative_frontend_paths_and_backend_only_ownership_are_explicit() {
    let fixture = Fixture::new();
    let relative = Path::new("前端 包含空格");
    assert!(check(&fixture.root, Some(relative)).unwrap().is_empty());
    fixture.write("前端 包含空格/src/index.ts", "ServiceAccount");
    assert_eq!(check(&fixture.root, Some(relative)).unwrap().len(), 1);
    assert!(check(&fixture.root, None).unwrap().is_empty());
    let invalid = fixture.root.join("不存在");
    assert!(
        check(&fixture.root, Some(&invalid))
            .unwrap_err()
            .to_string()
            .contains("--frontend-dir 必须指向现有前端项目")
    );
    fs::create_dir(&invalid).unwrap();
    assert!(check(&fixture.root, Some(&invalid)).is_err());
}

#[test]
fn required_file_io_and_invalid_utf8_fail_closed() {
    let fixture = Fixture::new();
    let path = fixture.root.join("openapi/openapi.json");
    fs::write(&path, [0xff, 0xfe]).unwrap();
    assert!(
        check(&fixture.root, None)
            .unwrap_err()
            .to_string()
            .contains("无法读取产品文件")
    );
    fs::remove_file(&path).unwrap();
    assert!(check(&fixture.root, None).is_err());
    fs::create_dir(&path).unwrap();
    assert!(check(&fixture.root, None).is_err());
    fs::remove_dir(&path).unwrap();
    fs::write(&path, "{}").unwrap();
    let build = fixture.root.join("crates/api/build.rs");
    fs::create_dir(&build).unwrap();
    assert!(check(&fixture.root, None).is_err());
    fs::remove_dir(build).unwrap();
    fs::create_dir(fixture.frontend().join("playwright.invalid.ts")).unwrap();
    assert!(check(&fixture.root, Some(&fixture.frontend())).is_err());
}

#[test]
fn every_current_executor_rejects_residue_without_starting_python() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let mut state = CheckExecutionState::default();
    execute_registered_task(
        TaskExecutor::RemovedIdentity,
        TaskExecutionMode::Ci,
        &context,
        &mut state,
    )
    .unwrap();
    fixture.write("crates/api/src/lib.rs", "ServiceAccount");
    let modes = [
        CheckPlanMode::ExplicitFull,
        CheckPlanMode::Selected(VerifySelection::default()),
    ];
    for (executor, mode) in [
        (TaskExecutor::RemovedIdentity, TaskExecutionMode::Ci),
        (TaskExecutor::PolicyChecks, TaskExecutionMode::Ci),
        (
            TaskExecutor::PolicyChecks,
            TaskExecutionMode::Check(&modes[0]),
        ),
        (
            TaskExecutor::BackendPolicyChecks,
            TaskExecutionMode::Check(&modes[0]),
        ),
        (
            TaskExecutor::SmartPolicyChecks,
            TaskExecutionMode::Check(&modes[1]),
        ),
        (
            TaskExecutor::BackendSmartPolicyChecks,
            TaskExecutionMode::Check(&modes[1]),
        ),
    ] {
        let error = execute_registered_task(executor, mode, &context, &mut state).unwrap_err();
        assert!(
            error.to_string().contains("已移除的产品定义仍存在"),
            "{executor:?}: {error}"
        );
        assert_eq!(failure_exit_code(error.as_ref()), Some(1));
    }
    assert!(!fixture.root.join("target").exists());
}

#[test]
fn backend_executor_ignores_an_unselected_frontend_and_never_writes_artifacts() {
    let fixture = Fixture::new();
    let mut context = fixture.context();
    context.frontend_dir = fixture.root.join("不存在");
    context.include_frontend = false;
    let mut state = CheckExecutionState::default();
    execute_registered_task(
        TaskExecutor::RemovedIdentity,
        TaskExecutionMode::Ci,
        &context,
        &mut state,
    )
    .unwrap();
    context.include_frontend = true;
    let error = execute_registered_task(
        TaskExecutor::RemovedIdentity,
        TaskExecutionMode::Ci,
        &context,
        &mut state,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("--frontend-dir 必须指向现有前端项目")
    );
    assert!(!fixture.root.join("target").exists());
}
