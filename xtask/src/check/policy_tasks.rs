#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicyProfile {
    FullStatic,
    Smart,
    CiPreflight,
}

pub(crate) const STRICT_MIGRATION_HISTORY_ARGS: &[&str] = &["--require-frozen"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicyTaskInvocation {
    Python(&'static str),
    MigrationHistory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PolicyTask {
    pub(crate) id: &'static str,
    pub(crate) invocation: PolicyTaskInvocation,
    full_order: Option<u8>,
    smart_order: Option<u8>,
    ci_preflight_order: Option<u8>,
}

impl PolicyTask {
    const fn order(self, profile: PolicyProfile) -> Option<u8> {
        match profile {
            PolicyProfile::FullStatic => self.full_order,
            PolicyProfile::Smart => self.smart_order,
            PolicyProfile::CiPreflight => self.ci_preflight_order,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn python_script(self) -> Option<&'static str> {
        match self.invocation {
            PolicyTaskInvocation::Python(script) => Some(script),
            PolicyTaskInvocation::MigrationHistory => None,
        }
    }
}

pub(crate) const POLICY_TASKS: &[PolicyTask] = &[
    PolicyTask {
        id: "migration-history",
        invocation: PolicyTaskInvocation::MigrationHistory,
        full_order: None,
        smart_order: Some(1),
        ci_preflight_order: None,
    },
    PolicyTask {
        id: "supply-chain",
        invocation: PolicyTaskInvocation::Python("tools/python/check_supply_chain.py"),
        full_order: Some(6),
        smart_order: None,
        ci_preflight_order: None,
    },
];

pub(crate) fn policy_tasks(profile: PolicyProfile) -> Vec<&'static PolicyTask> {
    let mut tasks = POLICY_TASKS
        .iter()
        .filter(|task| task.order(profile).is_some())
        .collect::<Vec<_>>();
    tasks.sort_by_key(|task| task.order(profile));
    tasks
}
