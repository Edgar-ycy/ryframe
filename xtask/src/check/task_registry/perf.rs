use super::{TaskDefinition, TaskExecutor, TaskRepository, TaskStage, TaskWorkingDirectory};

pub(super) const TASKS: [TaskDefinition; 2] = [
    task_definition!(
        PerfCgroupRun,
        "perf.cgroup.run",
        "run_devex_cgroup",
        "在专属 cgroup v2 中验收完整后代峰值内存",
        Backend,
        Test,
        Backend,
        None,
        &["xtask Linux cgroup 后代内存用例"],
        &["Cargo target", "cgroup 验收证据", "本次专属 cgroup 子树"],
        &["Linux cgroup v2 memory controller"]
    ),
    task_definition!(
        PerfCgroupCleanup,
        "perf.cgroup.cleanup",
        "cleanup_devex_cgroup",
        "复核并精确清理本次登记的 cgroup 子树",
        Backend,
        Test,
        Backend,
        None,
        &[],
        &["cgroup 清理证据", "本次专属 cgroup 子树"],
        &["Linux cgroup v2 memory controller"]
    ),
];
