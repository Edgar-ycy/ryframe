use std::path::Path;

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicyProfile {
    FullStatic,
    Smart,
    CiPreflight,
}

pub(crate) const MIGRATION_HISTORY_SCRIPT: &str = "scripts/check_migration_history.py";
pub(crate) const STRICT_MIGRATION_HISTORY_ARGS: &[&str] =
    &[MIGRATION_HISTORY_SCRIPT, "--require-frozen"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PythonPolicyTask {
    pub(crate) id: &'static str,
    pub(crate) script: &'static str,
    full_order: Option<u8>,
    smart_order: Option<u8>,
    ci_preflight_order: Option<u8>,
    requires_frontend: bool,
}

impl PythonPolicyTask {
    const fn order(self, profile: PolicyProfile) -> Option<u8> {
        match profile {
            PolicyProfile::FullStatic => self.full_order,
            PolicyProfile::Smart => self.smart_order,
            PolicyProfile::CiPreflight => self.ci_preflight_order,
        }
    }

    pub(crate) fn arguments(self, root: &Path, frontend_dir: &Path) -> Result<Vec<String>> {
        let mut arguments = vec![self.script.to_owned()];
        if self.requires_frontend {
            let frontend_dir = if frontend_dir.is_absolute() {
                frontend_dir.to_path_buf()
            } else {
                root.join(frontend_dir)
            };
            let frontend_dir = std::path::absolute(&frontend_dir).map_err(|error| {
                format!(
                    "无法解析服务身份检查使用的前端目录 {}：{error}",
                    frontend_dir.display()
                )
            })?;
            let frontend_dir = frontend_dir
                .to_str()
                .ok_or("服务身份检查使用的前端目录不是有效 UTF-8")?;
            arguments.push("--frontend-dir".to_owned());
            arguments.push(frontend_dir.to_owned());
        }
        Ok(arguments)
    }
}

pub(crate) const PYTHON_POLICY_TASKS: &[PythonPolicyTask] = &[
    PythonPolicyTask {
        id: "architecture",
        script: "scripts/check_architecture.py",
        full_order: Some(0),
        smart_order: Some(0),
        ci_preflight_order: Some(2),
        requires_frontend: false,
    },
    PythonPolicyTask {
        id: "deployment-assets",
        script: "scripts/check_deployment_assets.py",
        full_order: Some(1),
        smart_order: None,
        ci_preflight_order: Some(5),
        requires_frontend: false,
    },
    PythonPolicyTask {
        id: "migration-history",
        script: MIGRATION_HISTORY_SCRIPT,
        full_order: None,
        smart_order: Some(1),
        ci_preflight_order: None,
        requires_frontend: false,
    },
    PythonPolicyTask {
        id: "prerelease-dependencies",
        script: "scripts/check_prerelease_dependencies.py",
        full_order: Some(3),
        smart_order: None,
        ci_preflight_order: Some(0),
        requires_frontend: false,
    },
    PythonPolicyTask {
        id: "permission-routes",
        script: "scripts/check_permission_routes.py",
        full_order: Some(4),
        smart_order: Some(2),
        ci_preflight_order: Some(3),
        requires_frontend: false,
    },
    PythonPolicyTask {
        id: "removed-identity",
        script: "scripts/check_removed_identity.py",
        full_order: Some(5),
        smart_order: Some(3),
        ci_preflight_order: Some(4),
        requires_frontend: true,
    },
    PythonPolicyTask {
        id: "supply-chain",
        script: "scripts/check_supply_chain.py",
        full_order: Some(6),
        smart_order: None,
        ci_preflight_order: Some(1),
        requires_frontend: false,
    },
];

pub(crate) fn policy_tasks(profile: PolicyProfile) -> Vec<&'static PythonPolicyTask> {
    let mut tasks = PYTHON_POLICY_TASKS
        .iter()
        .filter(|task| task.order(profile).is_some())
        .collect::<Vec<_>>();
    tasks.sort_by_key(|task| task.order(profile));
    tasks
}
