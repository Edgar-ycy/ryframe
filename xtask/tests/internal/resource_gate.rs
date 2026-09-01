use std::collections::{BTreeMap, BTreeSet};

use super::{
    check::WorkspaceGraph,
    ci::resource_gate::{
        ChangeStatus, ChangedFile, GateStep, OwnershipEntry, OwnershipManifest, ResourceChangeSet,
        ResourceDefinition, ResourceGateInput, ResourceGateMode, affected_package_args_for_target,
        analyze, decision_for, enforce_targeted_activation, parse_name_status, parse_ownership,
        parse_resource_definition, plan_steps, preferred_nonempty_ref,
        resource_check_args_for_target, should_run_for_paths, targeted_activation_from,
        targeted_contract_steps, targeted_test_executables_from_messages, targeted_test_jobs_from,
        targeted_test_names_from_args, write_decision_artifact,
    },
};

#[path = "resource_gate_source.rs"]
mod resource_gate_source;
use resource_gate_source::source;
#[path = "resource_gate_execution.rs"]
mod resource_gate_execution;

const POST_PATH: &str = "catalog/resources/post.toml";

#[derive(Debug)]
struct ReplayCase {
    name: &'static str,
    input: ResourceGateInput,
    oracle: FullGateOracle,
}

#[derive(Debug)]
enum FullGateOracle {
    Targeted(ResourceChangeSet),
    FullFallback(&'static str),
}

#[test]
fn resource_change_replays_match_the_audited_full_gate_oracle() {
    let cases = resource_replay_cases();
    assert!(cases.len() >= 20, "resource gate 回放案例不得少于 20 个");
    let names = cases.iter().map(|case| case.name).collect::<BTreeSet<_>>();
    assert_eq!(names.len(), cases.len(), "resource gate 回放名称必须唯一");
    for category in [
        "新增/",
        "字段/",
        "权限/",
        "关系/",
        "SQL/",
        "重命名/",
        "删除/",
    ] {
        assert!(
            names.iter().any(|name| name.starts_with(category)),
            "resource gate 回放缺少 {category} 类案例"
        );
    }

    for case in cases {
        let actual = analyze(&case.input);
        assert_oracle_match(case.name, &actual, &case.oracle);
    }
}

#[test]
fn added_resource_uses_targeted_gate() {
    let mut input = input_with_post(None, &source("post", "title", "post.read", &[]));
    input.changes = vec![changed(ChangeStatus::Added, POST_PATH)];
    input
        .base_ownership
        .entries
        .retain(|entry| entry.resource != "post");

    let result = analyze(&input);

    assert_eq!(result.ambiguous_reason, None);
    assert_eq!(result.source_resources, set(&["post"]));
    assert!(result.schema_changed);
    assert!(result.permissions_changed);
    assert!(matches!(plan_steps(&result)[0], GateStep::ResourceDrift));
}

#[test]
fn field_change_marks_schema_and_affected_crate() {
    let base = source("post", "title", "post.read", &[]);
    let head = source("post", "body", "post.read", &[]);
    let input = input_with_post(Some(&base), &head);

    let result = analyze(&input);

    assert_eq!(result.ambiguous_reason, None);
    assert!(result.schema_changed);
    assert!(!result.permissions_changed);
    assert_eq!(result.affected_crates, set(&["ryframe-api"]));
}

#[test]
fn unchanged_catalog_output_does_not_expand_compile_surface() {
    let base = source("post", "title", "post.read", &[]);
    let head = source("post", "body", "post.read", &[]);
    let mut input = input_with_post(Some(&base), &head);
    let catalog_output = owned(
        "__catalog__",
        "crates/ryframe-tenant-db/src/generated/resources.rs",
    );
    input.base_ownership.entries.push(catalog_output.clone());
    input.head_ownership.entries.push(catalog_output);
    input.workspace_graph.package_by_dir.insert(
        "crates/ryframe-tenant-db".to_owned(),
        "ryframe-tenant-db".to_owned(),
    );

    let result = analyze(&input);

    assert_eq!(result.ambiguous_reason, None);
    assert_eq!(result.affected_crates, set(&["ryframe-api"]));
}

#[test]
fn unchanged_resource_output_does_not_expand_compile_surface() {
    let base = source("post", "title", "post.read", &[]);
    let head = source("post", "body", "post.read", &[]);
    let mut input = input_with_post(Some(&base), &head);
    let stable_output = owned(
        "post",
        "crates/ryframe-application/src/generated/post/model.rs",
    );
    input.base_ownership.entries.push(stable_output.clone());
    input.head_ownership.entries.push(stable_output);
    input.workspace_graph.package_by_dir.insert(
        "crates/ryframe-application".to_owned(),
        "ryframe-application".to_owned(),
    );

    let result = analyze(&input);

    assert_eq!(result.ambiguous_reason, None);
    assert_eq!(result.affected_crates, set(&["ryframe-api"]));
}

#[test]
fn standard_resource_does_not_compile_reverse_only_adapters() {
    let base = source("post", "title", "post.read", &[]);
    let head = source("post", "body", "post.read", &[]);
    let mut input = input_with_post(Some(&base), &head);
    input.workspace_graph.package_by_dir.insert(
        "crates/ryframe-adapters".to_owned(),
        "ryframe-adapters".to_owned(),
    );
    input
        .workspace_graph
        .reverse_dependencies
        .insert("ryframe-api".to_owned(), set(&["ryframe-adapters"]));

    let result = analyze(&input);

    assert_eq!(result.ambiguous_reason, None);
    assert_eq!(result.affected_crates, set(&["ryframe-api"]));
}

#[test]
fn control_resource_does_not_compile_reverse_only_tenant_database() {
    let base = source("post", "title", "post.read", &[]);
    let head = source("post", "body", "post.read", &[]);
    let mut input = input_with_post(Some(&base), &head);
    input.workspace_graph.package_by_dir.insert(
        "crates/ryframe-tenant-db".to_owned(),
        "ryframe-tenant-db".to_owned(),
    );
    input
        .workspace_graph
        .reverse_dependencies
        .insert("ryframe-api".to_owned(), set(&["ryframe-tenant-db"]));

    let result = analyze(&input);

    assert_eq!(result.ambiguous_reason, None);
    assert_eq!(result.affected_crates, set(&["ryframe-api"]));
}

#[test]
fn tenant_resource_keeps_direct_tenant_database_ownership() {
    let storage = "[storage]\nkind = \"tenant_data\"\ntenant_field = \"tenant_id\"";
    let base = with_section(&source("post", "tenant_id", "post.read", &[]), storage);
    let head = with_section(&source("post", "tenant_id", "post.list", &[]), storage);
    let mut input = input_with_post(Some(&base), &head);
    input.head_ownership.entries.push(owned(
        "post",
        "crates/ryframe-tenant-db/src/generated/post/mod.rs",
    ));
    input.workspace_graph.package_by_dir.insert(
        "crates/ryframe-tenant-db".to_owned(),
        "ryframe-tenant-db".to_owned(),
    );

    let result = analyze(&input);

    assert_eq!(result.ambiguous_reason, None);
    assert_eq!(
        result.affected_crates,
        set(&["ryframe-api", "ryframe-tenant-db"])
    );
}

#[test]
fn permission_change_marks_contract_impact() {
    let base = source("post", "title", "post.read", &[]);
    let head = source("post", "title", "post.list", &[]);
    let input = input_with_post(Some(&base), &head);

    let result = analyze(&input);

    assert_eq!(result.ambiguous_reason, None);
    assert!(result.permissions_changed);
    assert!(!result.schema_changed);
    assert!(plan_steps(&result).contains(&GateStep::PermissionContract));
    assert!(plan_steps(&result).contains(&GateStep::OpenApiAndFrontendConsumer));
}

#[test]
fn relation_change_uses_bidirectional_impact_closure() {
    let base = source("post", "notice_id", "post.read", &[]);
    let head = source("post", "notice_id", "post.read", &["notice"]);
    let input = input_with_post(Some(&base), &head);

    let result = analyze(&input);

    assert_eq!(result.ambiguous_reason, None);
    assert!(result.relations_changed);
    assert_eq!(result.impacted_resources, set(&["notice", "post"]));
    let serialized = serde_json::to_value(&result).unwrap();
    assert_eq!(
        serialized.get("impactedResources"),
        Some(&serde_json::json!(["notice", "post"]))
    );
    assert!(serialized.get("relationshipClosure").is_none());
}

#[test]
fn forbidden_sql_dsl_falls_back_to_full_gates() {
    let base = source("post", "title", "post.read", &[]);
    let mut head = source("post", "title", "post.read", &[]);
    head.push_str("\n[extensions.backend.report]\nquery_sql = \"SELECT 1\"\n");
    let input = input_with_post(Some(&base), &head);

    let result = analyze(&input);

    assert!(
        result
            .ambiguous_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("SQL DSL"))
    );
    assert_eq!(plan_steps(&result), full_fallback_steps());
}

#[test]
fn targeted_execution_stays_fail_closed_until_replay_activation() {
    let targeted = analyze(&input_with_post(
        Some(&source("post", "title", "post.read", &[])),
        &source("post", "body", "post.read", &[]),
    ));
    assert!(targeted.ambiguous_reason.is_none());

    let fail_closed = enforce_targeted_activation(targeted.clone(), false);
    assert!(
        fail_closed
            .ambiguous_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("replay 零分歧"))
    );
    assert_eq!(plan_steps(&fail_closed), full_fallback_steps());
    assert_eq!(
        enforce_targeted_activation(targeted.clone(), true),
        targeted
    );
    assert!(!targeted_activation_from(None));
    assert!(!targeted_activation_from(Some("1")));
    assert!(targeted_activation_from(Some("replay-verified-v1")));
}

#[test]
fn generator_and_unknown_generated_changes_force_full_scope() {
    let base = source("post", "title", "post.read", &[]);
    let mut generator = input_with_post(Some(&base), &base);
    generator.changes = vec![changed(
        ChangeStatus::Modified,
        "crates/ryframe-generator/src/resource/render.rs",
    )];
    assert!(
        analyze(&generator)
            .ambiguous_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("生成器"))
    );

    let mut generated = input_with_post(Some(&base), &base);
    generated.changes = vec![changed(
        ChangeStatus::Modified,
        "crates/ryframe-api/src/generated/unowned.rs",
    )];
    assert!(
        analyze(&generated)
            .ambiguous_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("ownership"))
    );
}

#[test]
fn relationship_and_workspace_thresholds_force_full_scope() {
    let base = source("post", "notice_id", "post.read", &[]);
    let head = source("post", "notice_id", "post.read", &["notice", "audit"]);
    let relation = input_with_post(Some(&base), &head);
    assert!(
        analyze(&relation)
            .ambiguous_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("50%"))
    );

    let base = source("post", "title", "post.read", &[]);
    let head = source("post", "body", "post.read", &[]);
    let mut workspace = input_with_post(Some(&base), &head);
    workspace
        .workspace_graph
        .package_by_dir
        .retain(|_, package| package == "ryframe-api");
    assert!(
        analyze(&workspace)
            .ambiguous_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("整个 Workspace"))
    );
}

#[test]
fn owned_output_threshold_forces_full_scope() {
    let base = source("post", "title", "post.read", &[]);
    let head = source("post", "body", "post.read", &[]);
    let mut input = input_with_post(Some(&base), &head);
    input.head_ownership.entries.extend((0..121).map(|index| {
        owned(
            "post",
            &format!("crates/ryframe-api/src/generated/post/{index}.rs"),
        )
    }));

    let result = analyze(&input);

    assert!(
        result
            .ambiguous_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("120"))
    );
}

#[test]
fn parses_git_status_and_rejects_unknown_status() {
    let parsed = parse_name_status("M\0a.rs\0R100\0old.rs\0new.rs\0").unwrap();
    assert_eq!(parsed[0], changed(ChangeStatus::Modified, "a.rs"));
    assert_eq!(parsed[1].status, ChangeStatus::Renamed);
    assert_eq!(parsed[1].old_path.as_deref(), Some("old.rs"));
    assert!(parse_name_status("T\0a.rs\0").is_err());
}

#[test]
fn ownership_parser_is_strict_and_commands_share_ci_target() {
    let ownership = "format_version = 1\ngenerator_version = \"1.2.0\"\n\
        [[entries]]\nresource = \"post\"\nroot = \"backend\"\npath = \"crates/x.rs\"\n\
        source_hash = \"s\"\ncontent_hash = \"c\"\n";
    assert_eq!(parse_ownership(ownership).unwrap().entries.len(), 1);
    assert!(parse_ownership(&format!("{ownership}unknown = true\n")).is_err());

    let changed_source = ownership.replace("source_hash = \"s\"", "source_hash = \"new\"");
    assert_eq!(
        parse_ownership(ownership).unwrap().entries[0].fingerprint,
        parse_ownership(&changed_source).unwrap().entries[0].fingerprint,
        "来源清单哈希不应让字节未变的生成文件进入 Rust 编译面"
    );
    let changed_content = ownership.replace("content_hash = \"c\"", "content_hash = \"new\"");
    assert_ne!(
        parse_ownership(ownership).unwrap().entries[0].fingerprint,
        parse_ownership(&changed_content).unwrap().entries[0].fingerprint
    );

    let packages = set(&["ryframe-api"]);
    let clippy =
        affected_package_args_for_target("clippy", &packages, "target/ci/backend", 3).unwrap();
    assert!(
        clippy
            .windows(2)
            .any(|pair| pair == ["--target-dir", "target/ci/backend"])
    );
    assert!(clippy.contains(&"--no-default-features".to_owned()));
    assert!(!clippy.contains(&"--all-features".to_owned()));
    assert!(!clippy.contains(&"--all-targets".to_owned()));
    assert!(clippy.windows(2).any(|pair| pair == ["--lib", "--bin"]));
    assert!(
        clippy
            .windows(2)
            .any(|pair| pair == ["--bin", "export_openapi"])
    );
    assert!(!clippy.contains(&"--test".to_owned()));
    let resource_args = resource_check_args_for_target(
        std::path::Path::new("../ryframe-vue3"),
        "target/ci/resource",
        2,
    );
    assert_eq!(
        resource_args,
        vec![
            "run".to_owned(),
            "--locked".to_owned(),
            "--target-dir".to_owned(),
            "target/ci/resource".to_owned(),
            "-p".to_owned(),
            "xtask".to_owned(),
            "--features".to_owned(),
            "resource".to_owned(),
            "--jobs".to_owned(),
            "2".to_owned(),
            "--".to_owned(),
            "resource".to_owned(),
            "--all".to_owned(),
            "--check".to_owned(),
            "--frontend-dir".to_owned(),
            "../ryframe-vue3".to_owned(),
        ]
    );
}

#[test]
fn targeted_test_artifacts_are_exact_and_complete() {
    let packages = set(&["ryframe-api", "ryframe-application", "ryframe-db"]);
    let args = affected_package_args_for_target("test", &packages, "target/ci/backend", 8).unwrap();
    let expected = targeted_test_names_from_args(&args).unwrap();
    assert_eq!(
        expected,
        set(&[
            "resource_api_contracts",
            "resource_application_contracts",
            "mapping_contracts",
        ])
    );

    let output = expected
        .iter()
        .map(|name| {
            format!(
                r#"{{"reason":"compiler-artifact","target":{{"name":"{name}"}},"executable":"D:/target/{name}.exe"}}"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let executables = targeted_test_executables_from_messages(&output, &expected).unwrap();
    assert_eq!(
        executables.keys().cloned().collect::<BTreeSet<_>>(),
        expected
    );

    assert!(
        targeted_test_executables_from_messages(output.lines().next().unwrap(), &expected)
            .unwrap_err()
            .to_string()
            .contains("缺少定向测试产物")
    );
    let duplicated = format!("{output}\n{output}");
    assert!(
        targeted_test_executables_from_messages(&duplicated, &expected)
            .unwrap_err()
            .to_string()
            .contains("重复返回定向测试产物")
    );
}

#[test]
fn ci_plan_selects_resource_and_full_fallback_surfaces_only() {
    for path in [
        "catalog/resources/post.toml",
        "catalog/resources/.ownership.toml",
        "crates/ryframe-api/src/generated/post/mod.rs",
        "crates/ryframe-generator/src/resource/render.rs",
        "Cargo.lock",
        ".github/workflows/ci.yml",
        "architecture/crate-boundaries.toml",
        "openapi/openapi.json",
    ] {
        assert!(should_run_for_paths(&[path.to_owned()]), "{path}");
    }
    for path in [
        "README.md",
        "docs/development.md",
        "crates/ryframe-api/src/handlers/auth.rs",
        "config/default.toml",
    ] {
        assert!(!should_run_for_paths(&[path.to_owned()]), "{path}");
    }
}

#[test]
fn configured_ci_ref_prefers_explicit_range_and_ignores_empty_values() {
    assert_eq!(
        preferred_nonempty_ref(Some("head"), Some("github")),
        Some("head".to_owned())
    );
    assert_eq!(
        preferred_nonempty_ref(Some("  "), Some("github")),
        Some("github".to_owned())
    );
    assert_eq!(preferred_nonempty_ref(None, Some("")), None);
}

fn resource_replay_cases() -> Vec<ReplayCase> {
    let mut cases = Vec::new();
    cases.extend(addition_and_field_replays());
    cases.extend(permission_replays());
    cases.extend(relation_replays());
    cases.extend(sql_and_lifecycle_replays());
    cases.extend(ownership_replays());
    cases.extend(threshold_and_build_replays());
    cases
}

fn addition_and_field_replays() -> Vec<ReplayCase> {
    let mut added = input_with_post(None, &source("post", "title", "post.read", &[]));
    added.changes = vec![changed(ChangeStatus::Added, POST_PATH)];
    remove_owner(&mut added.base_ownership, "post");

    let mut added_relation =
        input_with_post(None, &source("post", "notice_id", "post.read", &["notice"]));
    added_relation.changes = vec![changed(ChangeStatus::Added, POST_PATH)];
    remove_owner(&mut added_relation.base_ownership, "post");

    let field_name = input_with_post(
        Some(&source("post", "title", "post.read", &[])),
        &source("post", "body", "post.read", &[]),
    );
    let field_type = input_with_post(
        Some(&source("post", "title", "post.read", &[])),
        &source("post", "title", "post.read", &[])
            .replace("value_type = \"string\"", "value_type = \"integer\""),
    );
    let database_table = input_with_post(
        Some(&source("post", "title", "post.read", &[])),
        &source("post", "title", "post.read", &[])
            .replace("table = \"post\"", "table = \"sys_post\""),
    );

    vec![
        replay("新增/无关系资源", added, post_oracle(true, true, false)),
        replay(
            "新增/带单一关系资源",
            added_relation,
            related_post_oracle(true, true, true),
        ),
        replay(
            "字段/字段名变更",
            field_name,
            post_oracle(true, false, false),
        ),
        replay(
            "字段/字段类型变更",
            field_type,
            post_oracle(true, false, false),
        ),
        replay(
            "字段/数据库表名变更",
            database_table,
            post_oracle(true, false, false),
        ),
    ]
}

fn permission_replays() -> Vec<ReplayCase> {
    let base = source("post", "title", "post.read", &[]);
    let capability = input_with_post(Some(&base), &source("post", "title", "post.list", &[]));
    let api = input_with_post(
        Some(&base),
        &with_section(&base, "[api]\noperation = \"listPosts\"\n"),
    );
    let menu = input_with_post(
        Some(&base),
        &with_section(&base, "[menu]\nname = \"post\"\n"),
    );
    let route = input_with_post(
        Some(&base),
        &with_section(&base, "[route]\npath = \"/posts\"\n"),
    );
    let expected = || post_oracle(false, true, false);

    vec![
        replay("权限/access capability 变更", capability, expected()),
        replay("权限/API operation 变更", api, expected()),
        replay("权限/菜单声明变更", menu, expected()),
        replay("权限/路由声明变更", route, expected()),
    ]
}

fn relation_replays() -> Vec<ReplayCase> {
    let without_relation = source("post", "notice_id", "post.read", &[]);
    let with_relation = source("post", "notice_id", "post.read", &["notice"]);
    vec![
        replay(
            "关系/新增 belongs_to",
            input_with_post(Some(&without_relation), &with_relation),
            related_post_oracle(false, false, true),
        ),
        replay(
            "关系/删除 belongs_to",
            input_with_post(Some(&with_relation), &without_relation),
            related_post_oracle(false, false, true),
        ),
    ]
}

fn sql_and_lifecycle_replays() -> Vec<ReplayCase> {
    let base = source("post", "title", "post.read", &[]);
    let head = source("post", "body", "post.read", &[]);
    let mut sql_snapshot = input_with_post(Some(&base), &head);
    sql_snapshot
        .changes
        .push(changed(ChangeStatus::Modified, "sql/ryframe_config.sql"));

    let raw_sql = with_section(
        &base,
        "[extensions.backend.report]\nraw_sql = \"SELECT * FROM post\"\n",
    );
    let mut renamed_file = input_with_post(Some(&base), &base);
    renamed_file.changes = vec![ChangedFile {
        status: ChangeStatus::Renamed,
        path: "catalog/resources/article.toml".to_owned(),
        old_path: Some(POST_PATH.to_owned()),
    }];
    let renamed_resource = input_with_post(
        Some(&base),
        &base.replace("name = \"post\"", "name = \"article\""),
    );
    let mut deleted_source = input_with_post(Some(&base), &base);
    deleted_source.changes = vec![changed(ChangeStatus::Deleted, POST_PATH)];
    let mut deleted_output = input_with_post(Some(&base), &base);
    deleted_output.changes = vec![changed(
        ChangeStatus::Deleted,
        "crates/ryframe-api/src/generated/post/mod.rs",
    )];

    vec![
        replay(
            "SQL/生成 SQL 快照随字段变更",
            sql_snapshot,
            post_oracle(true, false, false),
        ),
        replay(
            "SQL/资源内嵌 raw_sql",
            input_with_post(Some(&base), &raw_sql),
            fallback("SQL DSL"),
        ),
        replay("重命名/Git 资源文件", renamed_file, fallback("重命名")),
        replay(
            "重命名/resource.name",
            renamed_resource,
            fallback("资源标识发生重命名"),
        ),
        replay("删除/资源事实源", deleted_source, fallback("删除")),
        replay("删除/ownership 受管输出", deleted_output, fallback("删除")),
    ]
}

fn ownership_replays() -> Vec<ReplayCase> {
    let base = source("post", "title", "post.read", &[]);
    let mut fingerprint = input_with_post(Some(&base), &base);
    fingerprint.changes = vec![changed(
        ChangeStatus::Modified,
        "catalog/resources/.ownership.toml",
    )];
    fingerprint.head_ownership.entries[0].fingerprint = "updated".to_owned();

    let mut frontend = input_with_post(Some(&base), &base);
    frontend.changes = vec![changed(
        ChangeStatus::Modified,
        "catalog/resources/.ownership.toml",
    )];
    frontend.head_ownership.entries.push(owned_in(
        "post",
        "frontend",
        "src/generated/resources/post/api.ts",
    ));

    let mut reassigned = input_with_post(Some(&base), &base);
    reassigned.changes = vec![changed(
        ChangeStatus::Modified,
        "catalog/resources/.ownership.toml",
    )];
    reassigned.head_ownership.entries[0].resource = "notice".to_owned();

    let mut duplicate = input_with_post(Some(&base), &base);
    duplicate.changes = vec![changed(
        ChangeStatus::Modified,
        "catalog/resources/.ownership.toml",
    )];
    duplicate.head_ownership.entries.push(owned(
        "post",
        "crates/ryframe-api/src/generated/post/mod.rs",
    ));

    let mut unknown_root = input_with_post(Some(&base), &base);
    unknown_root.changes = vec![changed(
        ChangeStatus::Modified,
        "catalog/resources/.ownership.toml",
    )];
    unknown_root.head_ownership.entries[0].root = "mobile".to_owned();

    let mut unowned = input_with_post(Some(&base), &base);
    unowned.changes = vec![changed(
        ChangeStatus::Modified,
        "crates/ryframe-api/src/generated/unowned.rs",
    )];

    let mut catalog_only = input_with_post(Some(&base), &base);
    catalog_only.changes = vec![changed(
        ChangeStatus::Modified,
        "crates/ryframe-api/src/generated/mod.rs",
    )];

    vec![
        replay(
            "ownership/指纹变更",
            fingerprint,
            post_oracle(false, false, false),
        ),
        replay(
            "ownership/新增前端受管输出",
            frontend,
            targeted_oracle(
                &["post"],
                &["post"],
                &backend_paths(&["post"]),
                &["src/generated/resources/post/api.ts"],
                &["ryframe-api"],
                (false, false, false),
            ),
        ),
        replay(
            "ownership/路径改归其他资源",
            reassigned,
            fallback("归属发生变化"),
        ),
        replay("ownership/重复路径", duplicate, fallback("重复路径")),
        replay("ownership/未知 root", unknown_root, fallback("未知 root")),
        replay("ownership/未登记生成输出", unowned, fallback("ownership")),
        replay(
            "ownership/仅聚合输出变化",
            catalog_only,
            fallback("聚合生成输出"),
        ),
    ]
}

fn threshold_and_build_replays() -> Vec<ReplayCase> {
    let base = source("post", "title", "post.read", &[]);
    let mut owned_outputs = input_with_post(Some(&base), &base);
    owned_outputs
        .head_ownership
        .entries
        .extend((0..121).map(|index| {
            owned(
                "post",
                &format!("crates/ryframe-api/src/generated/post/{index}.rs"),
            )
        }));

    let relation = input_with_post(
        Some(&source("post", "id", "post.read", &[])),
        &source("post", "id", "post.read", &["notice", "audit"]),
    );

    let mut workspace = input_with_post(Some(&base), &source("post", "body", "post.read", &[]));
    workspace
        .workspace_graph
        .reverse_dependencies
        .insert("ryframe-api".to_owned(), set(&["xtask"]));

    let mut subset = input_with_post(Some(&base), &source("post", "body", "post.read", &[]));
    subset
        .workspace_graph
        .package_by_dir
        .insert("crates/ryframe".to_owned(), "ryframe".to_owned());
    subset
        .workspace_graph
        .reverse_dependencies
        .insert("ryframe-api".to_owned(), set(&["ryframe"]));

    let mut generator = input_with_post(Some(&base), &base);
    generator.changes = vec![changed(
        ChangeStatus::Modified,
        "crates/ryframe-generator/src/resource/render.rs",
    )];
    let mut cargo = input_with_post(Some(&base), &base);
    cargo.changes = vec![changed(ChangeStatus::Modified, "Cargo.lock")];

    vec![
        replay(
            "阈值/九个资源事实源",
            many_added_resources(9),
            fallback("超过 8"),
        ),
        replay("阈值/一百二十一个受管输出", owned_outputs, fallback("120")),
        replay("阈值/关系闭包超过一半", relation, fallback("50%")),
        replay(
            "阈值/反向依赖覆盖完整 workspace",
            workspace,
            fallback("整个 Workspace"),
        ),
        replay(
            "阈值/反向依赖子闭包保持定向",
            subset,
            targeted_oracle(
                &["post"],
                &["post"],
                &backend_paths(&["post"]),
                &[],
                &["ryframe", "ryframe-api"],
                (true, false, false),
            ),
        ),
        replay("构建面/生成器实现变更", generator, fallback("生成器")),
        replay("构建面/Cargo lock 变更", cargo, fallback("Cargo")),
    ]
}

fn assert_oracle_match(name: &str, actual: &ResourceChangeSet, oracle: &FullGateOracle) {
    match oracle {
        FullGateOracle::Targeted(expected) => {
            assert_eq!(
                actual, expected,
                "回放 {name} 的影响集合与 full-gate oracle 不一致"
            );
            assert_eq!(
                plan_steps(actual),
                targeted_steps(&expected.affected_crates),
                "回放 {name} 的定向步骤未覆盖 full-gate oracle"
            );
        }
        FullGateOracle::FullFallback(reason) => {
            assert!(
                actual
                    .ambiguous_reason
                    .as_deref()
                    .is_some_and(|actual| actual.contains(reason)),
                "回放 {name} 应因 {reason} 回退，实际为 {:?}",
                actual.ambiguous_reason
            );
            assert_eq!(
                plan_steps(actual),
                full_fallback_steps(),
                "回放 {name} 未完整回退"
            );
        }
    }
}

fn full_fallback_steps() -> Vec<GateStep> {
    vec![
        GateStep::ResourceDrift,
        GateStep::FullRustGate,
        GateStep::FullIntegration,
        GateStep::PermissionContract,
        GateStep::MigrationContract,
        GateStep::FullConsumerContract,
    ]
}

fn replay(name: &'static str, input: ResourceGateInput, oracle: FullGateOracle) -> ReplayCase {
    ReplayCase {
        name,
        input,
        oracle,
    }
}

fn fallback(reason: &'static str) -> FullGateOracle {
    FullGateOracle::FullFallback(reason)
}

fn post_oracle(schema: bool, permissions: bool, relations: bool) -> FullGateOracle {
    targeted_oracle(
        &["post"],
        &["post"],
        &backend_paths(&["post"]),
        &[],
        &["ryframe-api"],
        (schema, permissions, relations),
    )
}

fn related_post_oracle(schema: bool, permissions: bool, relations: bool) -> FullGateOracle {
    targeted_oracle(
        &["post"],
        &["notice", "post"],
        &backend_paths(&["notice", "post"]),
        &[],
        &["ryframe-api"],
        (schema, permissions, relations),
    )
}

fn targeted_oracle(
    source: &[&str],
    closure: &[&str],
    backend: &[&str],
    frontend: &[&str],
    crates: &[&str],
    flags: (bool, bool, bool),
) -> FullGateOracle {
    FullGateOracle::Targeted(ResourceChangeSet {
        source_resources: set(source),
        impacted_resources: set(closure),
        owned_backend_paths: set(backend),
        owned_frontend_paths: set(frontend),
        affected_crates: set(crates),
        schema_changed: flags.0,
        permissions_changed: flags.1,
        relations_changed: flags.2,
        ambiguous_reason: None,
    })
}

fn targeted_steps(crates: &BTreeSet<String>) -> Vec<GateStep> {
    vec![
        GateStep::ResourceDrift,
        GateStep::ResourceWorkspace,
        GateStep::AffectedClippy(crates.clone()),
        GateStep::AffectedTest(crates.clone()),
        GateStep::PermissionContract,
        GateStep::MigrationContract,
        GateStep::OpenApiAndFrontendConsumer,
    ]
}

fn backend_paths(resources: &[&str]) -> Vec<&'static str> {
    let mut paths = Vec::new();
    for resource in resources {
        paths.push(match *resource {
            "post" => "crates/ryframe-api/src/generated/post/mod.rs",
            "notice" => "crates/ryframe-api/src/generated/notice/mod.rs",
            other => panic!("回放缺少 {other} 的受管路径"),
        });
    }
    paths
}

fn many_added_resources(count: usize) -> ResourceGateInput {
    let mut input = input_with_post(None, &source("post", "id", "read", &[]));
    input.changes.clear();
    input.base_resources.clear();
    input.head_resources.clear();
    input.base_ownership.entries.clear();
    input.head_ownership.entries.clear();
    for index in 0..count {
        let name = format!("resource_{index}");
        let path = format!("catalog/resources/{name}.toml");
        input.changes.push(changed(ChangeStatus::Added, &path));
        input.head_resources.insert(
            path.clone(),
            parse_resource_definition(&path, &source(&name, "id", "read", &[])).unwrap(),
        );
        input.head_ownership.entries.push(owned(
            &name,
            &format!("crates/ryframe-api/src/generated/{name}/mod.rs"),
        ));
    }
    input
}

fn remove_owner(ownership: &mut OwnershipManifest, resource: &str) {
    ownership.entries.retain(|entry| entry.resource != resource);
}

fn with_section(source: &str, section: &str) -> String {
    format!("{source}\n{section}")
}

fn input_with_post(base_source: Option<&str>, head_source: &str) -> ResourceGateInput {
    let mut base_resources = auxiliary_resources();
    if let Some(source) = base_source {
        base_resources.insert(
            POST_PATH.to_owned(),
            parse_resource_definition(POST_PATH, source).unwrap(),
        );
    }
    let mut head_resources = auxiliary_resources();
    head_resources.insert(
        POST_PATH.to_owned(),
        parse_resource_definition(POST_PATH, head_source).unwrap(),
    );
    let base_ownership = ownership();
    let mut head_ownership = ownership();
    if base_source != Some(head_source) {
        head_ownership
            .entries
            .iter_mut()
            .find(|entry| entry.resource == "post")
            .expect("测试 ownership 应包含 post")
            .fingerprint = "changed".to_owned();
    }
    ResourceGateInput {
        changes: vec![changed(ChangeStatus::Modified, POST_PATH)],
        base_resources,
        head_resources,
        base_ownership,
        head_ownership,
        workspace_graph: WorkspaceGraph {
            package_by_dir: [
                ("crates/ryframe-api".to_owned(), "ryframe-api".to_owned()),
                ("xtask".to_owned(), "xtask".to_owned()),
            ]
            .into_iter()
            .collect(),
            reverse_dependencies: BTreeMap::new(),
        },
    }
}

fn auxiliary_resources() -> BTreeMap<String, ResourceDefinition> {
    ["notice", "audit", "schedule"]
        .into_iter()
        .map(|name| {
            let path = format!("catalog/resources/{name}.toml");
            let definition =
                parse_resource_definition(&path, &source(name, "id", "read", &[])).unwrap();
            (path, definition)
        })
        .collect()
}

fn ownership() -> OwnershipManifest {
    OwnershipManifest {
        format_version: 1,
        generator_version: "1.2.0".to_owned(),
        entries: vec![
            owned("post", "crates/ryframe-api/src/generated/post/mod.rs"),
            owned("notice", "crates/ryframe-api/src/generated/notice/mod.rs"),
            owned("audit", "crates/ryframe-api/src/generated/audit/mod.rs"),
            owned(
                "schedule",
                "crates/ryframe-api/src/generated/schedule/mod.rs",
            ),
            owned("__catalog__", "crates/ryframe-api/src/generated/mod.rs"),
        ],
    }
}

fn owned(resource: &str, path: &str) -> OwnershipEntry {
    owned_in(resource, "backend", path)
}

fn owned_in(resource: &str, root: &str, path: &str) -> OwnershipEntry {
    OwnershipEntry {
        resource: resource.to_owned(),
        root: root.to_owned(),
        path: path.to_owned(),
        fingerprint: "stable".to_owned(),
    }
}

fn changed(status: ChangeStatus, path: &str) -> ChangedFile {
    ChangedFile {
        status,
        path: path.to_owned(),
        old_path: None,
    }
}

fn set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}
