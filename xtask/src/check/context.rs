use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
    thread,
};

use crate::{Result, workspace::root_dir};

/// 本地智能门禁默认复用日常开发的 Cargo 产物。
pub(crate) const BACKEND_SMART_TARGET_DIR: &str = "target";
/// 本地完整门禁使用独立且稳定的 Cargo 产物目录。
pub(crate) const BACKEND_VERIFY_TARGET_DIR: &str = "target/verify/backend";
/// CI 后端门禁使用的 Cargo 产物目录。
pub(crate) const BACKEND_CI_TARGET_DIR: &str = "target/ci/backend";
/// 本地完整门禁的临时资源 Workspace 产物目录。
pub(crate) const RESOURCE_VERIFY_TARGET_DIR: &str = "target/verify/resource";
/// CI 临时资源 Workspace 使用的 Cargo 产物目录。
pub(crate) const RESOURCE_CI_TARGET_DIR: &str = "target/ci/resource";
/// DevEx 资源门禁为每个样本注入的隔离 target 根目录。
pub(crate) const DEVEX_TARGET_ROOT_ENV: &str = "RYFRAME_DEVEX_TARGET_ROOT";

/// 单次 verify 内所有 Cargo 子命令共享的 target 选择。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifyTargetPolicy {
    pub(crate) backend: String,
    pub(crate) resource: String,
}

impl Default for VerifyTargetPolicy {
    fn default() -> Self {
        verify_target_policy_from(false, false, None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VerifyJobBudget {
    pub(crate) total: usize,
    pub(crate) backend: usize,
    pub(crate) resource: usize,
}

/// 单次 verify 固定根目录、并发预算和 target 策略，避免各阶段重新探测后产生漂移。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifyExecutionContext {
    pub(crate) root: PathBuf,
    pub(crate) frontend_dir: PathBuf,
    pub(crate) jobs: VerifyJobBudget,
    pub(crate) targets: VerifyTargetPolicy,
    ci: bool,
}

impl VerifyExecutionContext {
    pub(super) fn new(frontend_dir: &Path, full: bool) -> Result<Self> {
        // 与前端 Playwright 配置一致，以非空 CI 环境变量作为 CI 判定。
        let ci_value = env::var_os("CI");
        let ci = ci_environment_from(ci_value.as_deref());
        let cargo_target = env::var_os("CARGO_TARGET_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Ok(Self {
            root: root_dir(),
            frontend_dir: frontend_dir.to_path_buf(),
            jobs: verify_job_budget()?,
            targets: verify_target_policy_from(full, ci, cargo_target.as_deref()),
            ci,
        })
    }

    pub(crate) fn new_ci(frontend_dir: &Path) -> Result<Self> {
        let mut context = Self::new(frontend_dir, true)?;
        context.targets = ci_target_policy()?;
        context.ci = true;
        Ok(context)
    }

    /// 智能门禁因共享面变更扩大为完整门禁时，立即切换到完整 target 策略。
    pub(super) fn promote_to_full(&mut self) {
        self.targets = verify_target_policy_from(true, self.ci, None);
    }
}

pub(crate) fn ci_environment_from(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

pub(crate) fn verify_target_policy_from(
    full: bool,
    ci: bool,
    cargo_target_dir: Option<&Path>,
) -> VerifyTargetPolicy {
    if ci {
        return VerifyTargetPolicy {
            backend: BACKEND_CI_TARGET_DIR.to_owned(),
            resource: RESOURCE_CI_TARGET_DIR.to_owned(),
        };
    }
    if full {
        return VerifyTargetPolicy {
            backend: BACKEND_VERIFY_TARGET_DIR.to_owned(),
            resource: RESOURCE_VERIFY_TARGET_DIR.to_owned(),
        };
    }
    VerifyTargetPolicy {
        backend: cargo_target_dir
            .unwrap_or_else(|| Path::new(BACKEND_SMART_TARGET_DIR))
            .to_string_lossy()
            .into_owned(),
        resource: RESOURCE_VERIFY_TARGET_DIR.to_owned(),
    }
}

/// CI 子门禁默认复用稳定 target；DevEx 显式注入时改用样本级隔离目录。
pub(crate) fn ci_target_policy() -> Result<VerifyTargetPolicy> {
    let override_root = env::var_os(DEVEX_TARGET_ROOT_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    if override_root
        .as_ref()
        .is_some_and(|root| !root.is_absolute())
    {
        return Err(format!("{DEVEX_TARGET_ROOT_ENV} 必须是绝对路径").into());
    }
    Ok(ci_target_policy_from(override_root.as_deref()))
}

pub(crate) fn ci_target_policy_from(override_root: Option<&Path>) -> VerifyTargetPolicy {
    override_root.map_or_else(
        || VerifyTargetPolicy {
            backend: BACKEND_CI_TARGET_DIR.to_owned(),
            resource: RESOURCE_CI_TARGET_DIR.to_owned(),
        },
        |root| VerifyTargetPolicy {
            backend: root.join("backend").to_string_lossy().into_owned(),
            resource: root.join("resource").to_string_lossy().into_owned(),
        },
    )
}

/// Cargo 相对 target 以 Workspace 为基准；绝对 target 保持原路径用于缓存统计。
pub(crate) fn resolve_target_dir(root: &Path, target_dir: &str) -> PathBuf {
    let target_dir = Path::new(target_dir);
    if target_dir.is_absolute() {
        target_dir.to_path_buf()
    } else {
        root.join(target_dir)
    }
}

pub(crate) fn verify_job_budget_from(
    override_value: Option<&str>,
    available_parallelism: usize,
) -> Result<VerifyJobBudget> {
    let total = match override_value {
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|value| (4..=64).contains(value))
            .ok_or("RYFRAME_VERIFY_JOBS 必须是 4 到 64 的整数")?,
        None => available_parallelism.saturating_sub(2).clamp(4, 12),
    };
    let backend = ((total * 2).div_ceil(3)).clamp(2, total - 2);
    Ok(VerifyJobBudget {
        total,
        backend,
        resource: total - backend,
    })
}

pub(super) fn verify_job_budget() -> Result<VerifyJobBudget> {
    let available = thread::available_parallelism().map_or(4, usize::from);
    let configured = env::var("RYFRAME_VERIFY_JOBS").ok();
    verify_job_budget_from(configured.as_deref(), available)
}
