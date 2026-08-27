use std::collections::{BTreeMap, BTreeSet};

use super::{
    check::WorkspaceGraph,
    ci::resource_gate::{
        ChangeStatus, ChangedFile, GateStep, OwnershipEntry, OwnershipManifest, ResourceDefinition,
        ResourceGateInput, affected_package_args, analyze, parse_name_status, parse_ownership,
        parse_resource_definition, plan_steps, resource_check_args,
    },
};

const POST_PATH: &str = "catalog/resources/post.toml";

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
    assert_eq!(result.relationship_closure, set(&["notice", "post"]));
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
    assert_eq!(
        plan_steps(&result),
        vec![GateStep::FullRustGate, GateStep::FullConsumerContract]
    );
}

#[test]
fn resource_rename_falls_back_to_full_gates() {
    let base = source("post", "title", "post.read", &[]);
    let mut input = input_with_post(Some(&base), &base);
    input.changes = vec![ChangedFile {
        status: ChangeStatus::Renamed,
        path: "catalog/resources/article.toml".to_owned(),
        old_path: Some(POST_PATH.to_owned()),
    }];

    let result = analyze(&input);

    assert!(
        result
            .ambiguous_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("重命名"))
    );
}

#[test]
fn resource_delete_falls_back_to_full_gates() {
    let base = source("post", "title", "post.read", &[]);
    let mut input = input_with_post(Some(&base), &base);
    input.changes = vec![changed(ChangeStatus::Deleted, POST_PATH)];

    let result = analyze(&input);

    assert!(
        result
            .ambiguous_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("删除"))
    );
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

    let packages = set(&["ryframe-api"]);
    let clippy = affected_package_args("clippy", &packages, 3);
    assert!(
        clippy
            .windows(2)
            .any(|pair| pair == ["--target-dir", "target/ci/backend"])
    );
    assert!(clippy.contains(&"--all-features".to_owned()));
    assert_eq!(
        resource_check_args(std::path::Path::new("../ryframe-vue3")),
        [
            "resource",
            "--all",
            "--check",
            "--frontend-dir",
            "../ryframe-vue3"
        ]
        .map(str::to_owned)
    );
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
    ResourceGateInput {
        changes: vec![changed(ChangeStatus::Modified, POST_PATH)],
        base_resources,
        head_resources,
        base_ownership: ownership(),
        head_ownership: ownership(),
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
    OwnershipEntry {
        resource: resource.to_owned(),
        root: "backend".to_owned(),
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

fn source(name: &str, field: &str, permission: &str, relations: &[&str]) -> String {
    let relations = relations
        .iter()
        .map(|target| {
            format!(
                "[[relations]]\nname = \"{target}\"\nkind = \"belongs_to\"\nlocal_field = \"{field}\"\ntarget_resource = \"{target}\"\n"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "schema_version = 1\n\
         [[fields]]\nname = \"{field}\"\nvalue_type = \"string\"\norder = 1\n\
         {relations}\n\
         [resource]\nname = \"{name}\"\nmodule = \"{name}\"\nprofile = \"system_control\"\n\
         [database]\ntable = \"{name}\"\nprimary_key = [\"{field}\"]\n\
         [access]\ncapability = \"{permission}\"\n"
    )
}
