use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde::Deserialize;

use crate::Result;

const POLICY_PATH: &str = "architecture/change-surface.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ChangeCategory {
    HandwrittenProduct,
    Test,
    Generated,
    Migration,
    Documentation,
    Tooling,
    Other,
}

impl ChangeCategory {
    const ALL: [Self; 7] = [
        Self::HandwrittenProduct,
        Self::Test,
        Self::Generated,
        Self::Migration,
        Self::Documentation,
        Self::Tooling,
        Self::Other,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::HandwrittenProduct => "手写产品",
            Self::Test => "测试",
            Self::Generated => "生成",
            Self::Migration => "迁移",
            Self::Documentation => "文档",
            Self::Tooling => "工具",
            Self::Other => "其他",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RepositoryKind {
    Backend,
    Frontend,
}

impl RepositoryKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Backend => "后端",
            Self::Frontend => "前端",
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct WarningBudgets {
    pub(crate) backend_handwritten_product: usize,
    pub(crate) frontend_handwritten_product: usize,
    pub(crate) combined_handwritten_product: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct SoftSourceSize {
    pub(crate) backend_rust: usize,
    pub(crate) frontend_composable: usize,
    pub(crate) frontend_sfc_or_style: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct CentralHotspot {
    pub(crate) repository: RepositoryKind,
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) standard_resource_forbidden: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct ChangeSurfacePolicy {
    version: u32,
    pub(crate) warning_budgets: WarningBudgets,
    pub(crate) soft_source_size: SoftSourceSize,
    #[serde(default)]
    pub(crate) central_hotspots: Vec<CentralHotspot>,
}

impl ChangeSurfacePolicy {
    fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(format!("修改扩散配置版本不受支持：{}", self.version).into());
        }
        let budgets = &self.warning_budgets;
        if budgets.backend_handwritten_product == 0
            || budgets.frontend_handwritten_product == 0
            || budgets.combined_handwritten_product == 0
        {
            return Err("修改扩散预算必须大于零".into());
        }
        let sizes = &self.soft_source_size;
        if sizes.backend_rust == 0
            || sizes.frontend_composable == 0
            || sizes.frontend_sfc_or_style == 0
        {
            return Err("修改文件源码规模提醒阈值必须大于零".into());
        }
        let mut paths = BTreeSet::new();
        for hotspot in &self.central_hotspots {
            if hotspot.path.is_empty() || hotspot.path.contains('\\') {
                return Err(
                    format!("中央热点路径必须使用非空正斜杠相对路径：{}", hotspot.path).into(),
                );
            }
            if !paths.insert((hotspot.repository, hotspot.path.as_str())) {
                return Err(format!("中央热点重复配置：{}", hotspot.path).into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RepositoryChangeSurface {
    pub(crate) files: BTreeMap<ChangeCategory, Vec<String>>,
}

impl RepositoryChangeSurface {
    pub(crate) fn count(&self, category: ChangeCategory) -> usize {
        self.files.get(&category).map_or(0, Vec::len)
    }

    fn push(&mut self, category: ChangeCategory, path: &str) {
        self.files
            .entry(category)
            .or_default()
            .push(path.to_owned());
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChangeSurfaceReport {
    pub(crate) backend: RepositoryChangeSurface,
    pub(crate) frontend: RepositoryChangeSurface,
    pub(crate) domains: BTreeSet<String>,
    pub(crate) central_hotspots: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) violations: Vec<String>,
}

pub(crate) fn load_change_surface_policy(root: &Path) -> Result<ChangeSurfacePolicy> {
    let path = root.join(POLICY_PATH);
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("读取修改扩散配置 {} 失败：{error}", path.display()))?;
    let policy = toml::from_str::<ChangeSurfacePolicy>(&source)
        .map_err(|error| format!("解析修改扩散配置 {} 失败：{error}", path.display()))?;
    policy.validate()?;
    Ok(policy)
}

pub(crate) fn analyze_change_surface(
    backend_paths: &[String],
    frontend_paths: &[String],
    policy: &ChangeSurfacePolicy,
) -> ChangeSurfaceReport {
    let mut report = ChangeSurfaceReport::default();
    for path in backend_paths {
        report.backend.push(classify_backend(path), path);
        if let Some(domain) = backend_domain(path) {
            report.domains.insert(domain);
        }
    }
    for path in frontend_paths {
        report.frontend.push(classify_frontend(path), path);
        if let Some(domain) = frontend_domain(path) {
            report.domains.insert(domain);
        }
    }

    let standard_resource_change = backend_paths.iter().any(|path| {
        path.starts_with("catalog/resources/")
            && path.ends_with(".toml")
            && path != "catalog/resources/.ownership.toml"
    });
    for hotspot in &policy.central_hotspots {
        let changed = match hotspot.repository {
            RepositoryKind::Backend => backend_paths.contains(&hotspot.path),
            RepositoryKind::Frontend => frontend_paths.contains(&hotspot.path),
        };
        if changed {
            let label = format!("{}:{}", hotspot.repository.label(), hotspot.path);
            report.central_hotspots.push(label.clone());
            if standard_resource_change && hotspot.standard_resource_forbidden {
                report.violations.push(format!(
                    "标准资源变更不得手工修改中央热点 {label}；请补齐生成器或领域聚合入口"
                ));
            }
        }
    }

    let backend_handwritten = report.backend.count(ChangeCategory::HandwrittenProduct);
    let frontend_handwritten = report.frontend.count(ChangeCategory::HandwrittenProduct);
    let budgets = &policy.warning_budgets;
    append_budget_warning(
        &mut report.warnings,
        "后端手写产品文件",
        backend_handwritten,
        budgets.backend_handwritten_product,
    );
    append_budget_warning(
        &mut report.warnings,
        "前端手写产品文件",
        frontend_handwritten,
        budgets.frontend_handwritten_product,
    );
    append_budget_warning(
        &mut report.warnings,
        "前后端手写产品文件合计",
        backend_handwritten + frontend_handwritten,
        budgets.combined_handwritten_product,
    );
    report
}

pub(crate) fn append_changed_file_size_warnings(
    backend_root: &Path,
    frontend_root: &Path,
    backend_paths: &[String],
    frontend_paths: &[String],
    policy: &ChangeSurfacePolicy,
    report: &mut ChangeSurfaceReport,
) -> Result<()> {
    for path in backend_paths {
        if !path.ends_with(".rs")
            || matches!(
                classify_backend(path),
                ChangeCategory::Generated | ChangeCategory::Test
            )
        {
            continue;
        }
        append_file_size_warning(
            &mut report.warnings,
            backend_root,
            path,
            policy.soft_source_size.backend_rust,
            "Rust 源码",
        )?;
    }
    for path in frontend_paths {
        let file_name = Path::new(path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let (limit, label) = if path.ends_with(".ts")
            && file_name.starts_with("use")
            && !path.ends_with(".test.ts")
            && !path.ends_with(".spec.ts")
        {
            (policy.soft_source_size.frontend_composable, "Composable")
        } else if path.ends_with(".vue") || path.ends_with(".scss") {
            (policy.soft_source_size.frontend_sfc_or_style, "SFC/SCSS")
        } else {
            continue;
        };
        append_file_size_warning(&mut report.warnings, frontend_root, path, limit, label)?;
    }
    Ok(())
}

fn append_file_size_warning(
    warnings: &mut Vec<String>,
    root: &Path,
    relative: &str,
    limit: usize,
    label: &str,
) -> Result<()> {
    let path = root.join(relative);
    if !path.is_file() {
        return Ok(());
    }
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("读取修改文件 {} 失败：{error}", path.display()))?;
    let lines = source.lines().count();
    if lines > limit {
        warnings.push(format!(
            "{label} {relative} 共 {lines} 行，超过软提醒阈值 {limit} 行"
        ));
    }
    Ok(())
}

pub(super) fn print_change_surface(report: &ChangeSurfaceReport) {
    println!("修改扩散统计：");
    print_repository_surface(RepositoryKind::Backend, &report.backend);
    print_repository_surface(RepositoryKind::Frontend, &report.frontend);
    if !report.domains.is_empty() {
        println!(
            "  涉及领域：{}",
            report
                .domains
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join("、")
        );
    }
    if !report.central_hotspots.is_empty() {
        println!("  中央热点：{}", report.central_hotspots.join("、"));
    }
    for warning in &report.warnings {
        println!("  预算提醒：{warning}");
    }
    for violation in &report.violations {
        println!("  门禁失败：{violation}");
    }
}

pub(super) fn enforce_change_surface(report: &ChangeSurfaceReport) -> Result<()> {
    if report.violations.is_empty() {
        return Ok(());
    }
    Err(format!("修改扩散门禁发现 {} 个问题", report.violations.len()).into())
}

fn print_repository_surface(repository: RepositoryKind, surface: &RepositoryChangeSurface) {
    let summary = ChangeCategory::ALL
        .into_iter()
        .filter_map(|category| {
            let count = surface.count(category);
            (count > 0).then(|| format!("{}={count}", category.label()))
        })
        .collect::<Vec<_>>();
    println!(
        "  {}：{}",
        repository.label(),
        if summary.is_empty() {
            "无变更".to_owned()
        } else {
            summary.join("，")
        }
    );
}

fn append_budget_warning(warnings: &mut Vec<String>, label: &str, actual: usize, budget: usize) {
    if actual > budget {
        warnings.push(format!("{label} {actual} 个，超过提醒预算 {budget} 个"));
    }
}

fn classify_backend(path: &str) -> ChangeCategory {
    if is_documentation(path) {
        ChangeCategory::Documentation
    } else if path.contains("/src/generated/")
        || path == "catalog/access.generated.toml"
        || path == "catalog/resources/.ownership.toml"
        || path == "openapi/openapi.json"
        || path == "sql/ryframe_config.sql"
        || path.contains(".generated.")
    {
        ChangeCategory::Generated
    } else if path.contains("/migration/") || path == "catalog/migrations.lock.toml" {
        ChangeCategory::Migration
    } else if is_test(path) {
        ChangeCategory::Test
    } else if path.starts_with("crates/")
        && !path.starts_with("crates/ryframe-generator/")
        && !path.starts_with("crates/ryframe-macro/")
        && (path.contains("/src/") || path.ends_with("/build.rs"))
        || path.starts_with("catalog/resources/")
        || path == "catalog/access.toml"
    {
        ChangeCategory::HandwrittenProduct
    } else if path.starts_with("xtask/")
        || path.starts_with("scripts/")
        || path.starts_with("architecture/")
        || path.starts_with("crates/ryframe-generator/")
        || path.starts_with("crates/ryframe-macro/")
        || path.starts_with(".github/")
    {
        ChangeCategory::Tooling
    } else {
        ChangeCategory::Other
    }
}

fn classify_frontend(path: &str) -> ChangeCategory {
    if is_documentation(path) {
        ChangeCategory::Documentation
    } else if path.starts_with("openapi/")
        || path.starts_with("src/api/generated/")
        || path.starts_with("src/generated/")
        || path.contains(".generated.")
    {
        ChangeCategory::Generated
    } else if is_test(path) {
        ChangeCategory::Test
    } else if path.starts_with("src/") || path.starts_with("public/") {
        ChangeCategory::HandwrittenProduct
    } else if path.starts_with("scripts/")
        || path.starts_with(".github/")
        || path.starts_with("tests/")
        || path == "package.json"
        || path == "pnpm-lock.yaml"
        || path.contains(".config.")
        || path.starts_with("tsconfig")
    {
        ChangeCategory::Tooling
    } else {
        ChangeCategory::Other
    }
}

fn is_documentation(path: &str) -> bool {
    path.starts_with("docs/")
        || matches!(
            path,
            "README.md"
                | "ARCHITECTURE.md"
                | "CHANGELOG.md"
                | "CONTRIBUTING.md"
                | "SECURITY.md"
                | "CODE_OF_CONDUCT.md"
                | "LICENSE"
                | "LICENSE.md"
        )
}

fn is_test(path: &str) -> bool {
    path.starts_with("tests/")
        || path.starts_with("scripts/tests/")
        || path.contains("/tests/")
        || path.ends_with(".test.ts")
        || path.ends_with(".spec.ts")
        || path.ends_with("_test.rs")
        || path.ends_with("_tests.rs")
}

fn backend_domain(path: &str) -> Option<String> {
    let markers = [
        "/src/system/",
        "/src/ports/system/",
        "/src/handlers/",
        "/src/dto/",
        "/src/application_ports/system/",
        "/src/repositories/",
        "catalog/resources/",
    ];
    markers
        .into_iter()
        .find_map(|marker| path.split_once(marker).map(|(_, remainder)| remainder))
        .and_then(domain_name)
}

fn frontend_domain(path: &str) -> Option<String> {
    let remainder = path
        .strip_prefix("src/features/")
        .or_else(|| path.strip_prefix("src/views/"))
        .or_else(|| path.strip_prefix("src/api/modules/"))?;
    let mut segments = remainder.split('/');
    let first = segments.next()?;
    let candidate = if matches!(first, "system" | "monitor" | "tool") {
        segments.next().unwrap_or(first)
    } else {
        first
    };
    domain_name(candidate)
}

fn domain_name(value: &str) -> Option<String> {
    let candidate = value.split('/').next()?.split('.').next()?;
    let candidate = candidate
        .strip_suffix("_handler")
        .or_else(|| candidate.strip_suffix("_dto"))
        .or_else(|| candidate.strip_suffix("_repo"))
        .unwrap_or(candidate);
    (!candidate.is_empty() && !matches!(candidate, "mod" | "lib" | "main" | "generated"))
        .then(|| candidate.replace('_', "-"))
}
