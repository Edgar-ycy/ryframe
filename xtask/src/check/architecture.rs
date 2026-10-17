use std::path::Path;

use crate::{Result, process::PreservedFailure};

#[path = "architecture/ast.rs"]
pub(crate) mod ast;
#[path = "architecture/docs.rs"]
pub(crate) mod docs;
#[path = "architecture/policy.rs"]
pub(crate) mod policy;
#[path = "architecture/source_size.rs"]
pub(crate) mod source_size;
#[path = "architecture/support.rs"]
mod support;
#[path = "architecture/syntax.rs"]
pub(crate) mod syntax;
#[path = "architecture/tenant.rs"]
pub(crate) mod tenant;
#[path = "architecture/test_layout.rs"]
pub(crate) mod test_layout;
#[path = "architecture/workspace.rs"]
pub(crate) mod workspace;

pub(crate) fn run(root: &Path) -> Result<()> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    docs::validate_documentation(root, &mut errors);
    docs::validate_system_domain_surface(root, &mut errors);
    let policy = policy::load(root, &mut errors);
    let packages = workspace::load_packages(root, &mut errors);
    let unsafe_lint_packages = if packages.is_empty() {
        0
    } else {
        workspace::validate_unsafe_lint_policy(root, &packages, &mut errors)
    };
    let mut actual_edges = 0;
    let mut checked_test_sources = 0;
    let mut integration_targets = 0;
    let mut persistence_sources = 0;
    let mut legacy_persistence_violations = 0;
    let mut async_port_sources = 0;
    let mut async_port_violations = 0;
    let mut ast_stats = ast::AstStats::default();
    let mut scanned_by_package = 0;
    let mut scanned = 0;
    if let Some(active) = policy.profiles.get(&policy.active_profile)
        && !packages.is_empty()
    {
        actual_edges = workspace::validate_active_workspace(
            root,
            &policy.active_profile,
            active,
            &packages,
            &mut errors,
        )
        .len();
        let source_stats = source_size::validate_source_size(
            root,
            &policy.source_size,
            active,
            &packages,
            &mut errors,
            &mut warnings,
        );
        scanned = source_stats.scanned;
        scanned_by_package = source_stats.by_package.len();
        let (checked_sources, targets) =
            test_layout::validate_test_layout(root, &policy.test_layout, &packages, &mut errors);
        checked_test_sources = checked_sources;
        integration_targets = targets;
        let sources = syntax::workspace_rust_sources(root, active, &packages);
        (persistence_sources, legacy_persistence_violations) =
            syntax::validate_legacy_persistence_apis(root, &sources, &mut errors);
        (async_port_sources, async_port_violations) =
            syntax::validate_async_port_traits(root, &sources, &mut errors);
        let templates = syntax::workspace_rust_templates(root, active, &packages);
        ast_stats = ast::validate_ast(
            root,
            &sources,
            &templates,
            &policy.function_size,
            &source_stats.frozen_migrations,
            &mut errors,
        );
    }
    tenant::validate_tenant_data_boundaries(root, &mut errors);
    if !warnings.is_empty() {
        eprintln!("Architecture size notices:");
        for warning in warnings {
            eprintln!("  - {warning}");
        }
    }
    if !errors.is_empty() {
        eprintln!("Architecture check failed:");
        for error in errors {
            eprintln!("  - {error}");
        }
        return Err(PreservedFailure::new("架构检查失败".into(), Some(1)).into());
    }
    println!(
        "Architecture check passed (profile={}, packages={}, internal_edges={actual_edges}, scc=0).",
        policy.active_profile,
        packages.len()
    );
    let summaries = policy
        .profiles
        .iter()
        .map(|(name, profile)| format!("{name}={}", profile.packages.len()))
        .collect::<Vec<_>>()
        .join(", ");
    println!("Declared architecture profiles are valid ({summaries}).");
    println!(
        "Workspace source-size coverage passed (crates={scanned_by_package}, files={scanned}, max_lines={}).",
        policy
            .source_size
            .get("max_lines")
            .and_then(toml::Value::as_integer)
            .unwrap_or(0)
    );
    println!(
        "Crate test-layout coverage passed (source_files={checked_test_sources}, integration_targets={integration_targets})."
    );
    println!(
        "Legacy persistence API gate passed (source_files={persistence_sources}, violations={legacy_persistence_violations})."
    );
    println!(
        "Async port interface gate passed (source_files={async_port_sources}, violations={async_port_violations})."
    );
    println!(
        "Rust function-size AST gate passed (mode={}, functions={}, frozen_functions={}, violations={}).",
        policy
            .function_size
            .get("mode")
            .and_then(toml::Value::as_str)
            .unwrap_or(""),
        ast_stats.checked_functions,
        ast_stats.excluded_frozen_functions,
        ast_stats.function_violations
    );
    println!(
        "Rust unsafe AST gate passed (source_files={}, violations={}).",
        ast_stats.checked_unsafe_sources, ast_stats.unsafe_violations
    );
    println!("Workspace unsafe lint inheritance passed (packages={unsafe_lint_packages}).");
    println!("Tenant-data architecture boundaries are valid.");
    Ok(())
}
