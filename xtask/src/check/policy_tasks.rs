#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicyProfile {
    FullStatic,
    Smart,
    CiPreflight,
}

pub(crate) const MIGRATION_HISTORY_SCRIPT: &str = "tools/python/check_migration_history.py";
pub(crate) const STRICT_MIGRATION_HISTORY_ARGS: &[&str] =
    &[MIGRATION_HISTORY_SCRIPT, "--require-frozen"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PythonPolicyTask {
    pub(crate) id: &'static str,
    pub(crate) script: &'static str,
    full_order: Option<u8>,
    smart_order: Option<u8>,
    ci_preflight_order: Option<u8>,
}

impl PythonPolicyTask {
    const fn order(self, profile: PolicyProfile) -> Option<u8> {
        match profile {
            PolicyProfile::FullStatic => self.full_order,
            PolicyProfile::Smart => self.smart_order,
            PolicyProfile::CiPreflight => self.ci_preflight_order,
        }
    }

    pub(crate) fn arguments(self) -> Vec<String> {
        vec![self.script.to_owned()]
    }
}

pub(crate) const PYTHON_POLICY_TASKS: &[PythonPolicyTask] = &[
    PythonPolicyTask {
        id: "deployment-assets",
        script: "tools/python/check_deployment_assets.py",
        full_order: Some(1),
        smart_order: None,
        ci_preflight_order: None,
    },
    PythonPolicyTask {
        id: "migration-history",
        script: MIGRATION_HISTORY_SCRIPT,
        full_order: None,
        smart_order: Some(1),
        ci_preflight_order: None,
    },
    PythonPolicyTask {
        id: "permission-routes",
        script: "tools/python/check_permission_routes.py",
        full_order: Some(4),
        smart_order: Some(2),
        ci_preflight_order: Some(3),
    },
    PythonPolicyTask {
        id: "supply-chain",
        script: "tools/python/check_supply_chain.py",
        full_order: Some(6),
        smart_order: None,
        ci_preflight_order: None,
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
