#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicyProfile {
    Full,
    Smart,
    CiPreflight,
}

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
            PolicyProfile::Full => self.full_order,
            PolicyProfile::Smart => self.smart_order,
            PolicyProfile::CiPreflight => self.ci_preflight_order,
        }
    }
}

pub(crate) const PYTHON_POLICY_TASKS: &[PythonPolicyTask] = &[
    PythonPolicyTask {
        id: "architecture",
        script: "scripts/check_architecture.py",
        full_order: Some(0),
        smart_order: Some(0),
        ci_preflight_order: Some(2),
    },
    PythonPolicyTask {
        id: "deployment-assets",
        script: "scripts/check_deployment_assets.py",
        full_order: Some(1),
        smart_order: None,
        ci_preflight_order: Some(5),
    },
    PythonPolicyTask {
        id: "migration-history",
        script: "scripts/check_migration_history.py",
        full_order: Some(2),
        smart_order: Some(1),
        ci_preflight_order: None,
    },
    PythonPolicyTask {
        id: "prerelease-dependencies",
        script: "scripts/check_prerelease_dependencies.py",
        full_order: Some(3),
        smart_order: None,
        ci_preflight_order: Some(0),
    },
    PythonPolicyTask {
        id: "permission-routes",
        script: "scripts/check_permission_routes.py",
        full_order: Some(4),
        smart_order: Some(2),
        ci_preflight_order: Some(3),
    },
    PythonPolicyTask {
        id: "removed-identity",
        script: "scripts/check_removed_identity.py",
        full_order: Some(5),
        smart_order: Some(3),
        ci_preflight_order: Some(4),
    },
    PythonPolicyTask {
        id: "supply-chain",
        script: "scripts/check_supply_chain.py",
        full_order: Some(6),
        smart_order: None,
        ci_preflight_order: Some(1),
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
