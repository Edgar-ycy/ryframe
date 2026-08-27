use std::{collections::BTreeSet, fs, path::Path};

use serde::Deserialize;

use crate::Result;

const POLICY_PATH: &str = "architecture/change-surface.toml";

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RepositoryKind {
    Backend,
    Frontend,
}

impl RepositoryKind {
    pub(super) const fn label(self) -> &'static str {
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
    pub(crate) warning_percent: usize,
    pub(crate) attention_percent: usize,
    pub(crate) backend_rust_hard_limit: usize,
    pub(crate) frontend_typescript_hard_limit: usize,
    pub(crate) frontend_composable_hard_limit: usize,
    pub(crate) frontend_sfc_hard_limit: usize,
    pub(crate) frontend_style_hard_limit: usize,
    pub(crate) frontend_script_hard_limit: usize,
}

impl SoftSourceSize {
    pub(super) fn budget(&self, hard_limit: usize) -> SourceSizeBudget {
        SourceSizeBudget {
            warning: percent_of(hard_limit, self.warning_percent),
            attention: percent_of(hard_limit, self.attention_percent),
            hard_limit,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.warning_percent != 80 || self.attention_percent != 90 {
            return Err("修改文件源码规模软阈值必须固定为 80% 提醒和 90% 高关注".into());
        }
        for (label, hard_limit) in [
            ("后端 Rust", self.backend_rust_hard_limit),
            ("前端 TypeScript", self.frontend_typescript_hard_limit),
            ("前端 Composable", self.frontend_composable_hard_limit),
            ("前端 SFC", self.frontend_sfc_hard_limit),
            ("前端样式", self.frontend_style_hard_limit),
            ("前端脚本", self.frontend_script_hard_limit),
        ] {
            let budget = self.budget(hard_limit);
            if hard_limit == 0
                || budget.warning == 0
                || budget.warning >= budget.attention
                || budget.attention >= hard_limit
            {
                return Err(format!(
                    "{label} 源码规模阈值必须满足 0 < 80% 提醒 < 90% 高关注 < 硬上限"
                )
                .into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SourceSizeBudget {
    pub(super) warning: usize,
    pub(super) attention: usize,
    pub(super) hard_limit: usize,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PathMatchKind {
    Exact,
    Prefix,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct FullInvalidationPath {
    pub(crate) repository: RepositoryKind,
    #[serde(rename = "match")]
    pub(crate) match_kind: PathMatchKind,
    pub(crate) path: String,
    pub(crate) reason: String,
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
    pub(crate) full_invalidation_paths: Vec<FullInvalidationPath>,
    #[serde(default)]
    pub(crate) central_hotspots: Vec<CentralHotspot>,
}

impl ChangeSurfacePolicy {
    fn validate(&self) -> Result<()> {
        if self.version != 2 {
            return Err(format!("修改扩散配置版本不受支持：{}", self.version).into());
        }
        let budgets = &self.warning_budgets;
        if budgets.backend_handwritten_product == 0
            || budgets.frontend_handwritten_product == 0
            || budgets.combined_handwritten_product == 0
        {
            return Err("修改扩散预算必须大于零".into());
        }
        self.soft_source_size.validate()?;
        if self.full_invalidation_paths.is_empty() {
            return Err("完整失效路径不得为空".into());
        }
        let mut invalidation_paths = BTreeSet::new();
        for rule in &self.full_invalidation_paths {
            validate_invalidation_path(rule)?;
            if !invalidation_paths.insert((rule.repository, rule.match_kind, rule.path.as_str())) {
                return Err(format!("完整失效路径重复配置：{}", rule.path).into());
            }
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

    #[allow(dead_code, reason = "完整失效路径先落机器配置，后续由资源智能门禁消费")]
    pub(crate) fn full_invalidation_reason(
        &self,
        repository: RepositoryKind,
        path: &str,
    ) -> Option<&str> {
        self.full_invalidation_paths
            .iter()
            .find(|rule| {
                rule.repository == repository
                    && match rule.match_kind {
                        PathMatchKind::Exact => path == rule.path,
                        PathMatchKind::Prefix => path.starts_with(&rule.path),
                    }
            })
            .map(|rule| rule.reason.as_str())
    }
}

pub(crate) fn load_change_surface_policy(root: &Path) -> Result<ChangeSurfacePolicy> {
    let path = root.join(POLICY_PATH);
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("读取修改扩散配置 {} 失败：{error}", path.display()))?;
    parse_change_surface_policy(&source)
        .map_err(|error| format!("解析修改扩散配置 {} 失败：{error}", path.display()).into())
}

pub(crate) fn parse_change_surface_policy(source: &str) -> Result<ChangeSurfacePolicy> {
    let policy = toml::from_str::<ChangeSurfacePolicy>(source)
        .map_err(|error| format!("TOML 结构无效：{error}"))?;
    policy.validate()?;
    Ok(policy)
}

fn percent_of(limit: usize, percent: usize) -> usize {
    limit.saturating_mul(percent).div_ceil(100)
}

fn validate_invalidation_path(rule: &FullInvalidationPath) -> Result<()> {
    let path = rule.path.as_str();
    let normalized = match rule.match_kind {
        PathMatchKind::Exact if path.ends_with('/') => {
            return Err(format!("精确完整失效路径不得以斜杠结尾：{path}").into());
        }
        PathMatchKind::Prefix if !path.ends_with('/') => {
            return Err(format!("前缀完整失效路径必须以斜杠结尾：{path}").into());
        }
        PathMatchKind::Prefix => path.trim_end_matches('/'),
        PathMatchKind::Exact => path,
    };
    if normalized.is_empty()
        || path.contains('\\')
        || path.contains(':')
        || path.starts_with('/')
        || path.starts_with("./")
        || path.contains("//")
        || normalized
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Err(format!("完整失效路径必须是规范的正斜杠相对路径：{path}").into());
    }
    if rule.reason.trim().is_empty() {
        return Err(format!("完整失效路径必须说明原因：{path}").into());
    }
    Ok(())
}
