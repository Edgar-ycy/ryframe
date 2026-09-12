use super::{TaskDefinition, TaskExecutor, TaskRepository, TaskStage, TaskWorkingDirectory};

pub(super) const TASKS: [TaskDefinition; 2] = [
    task_definition!(
        ReleaseSource,
        "release.source",
        "verify_release_source",
        "核对双端 tag、提交、版本与 OpenAPI，并生成联合发布清单",
        CrossRepository,
        Contract,
        Backend,
        None,
        &[],
        &["显式指定的发布清单"],
        &[]
    ),
    task_definition!(
        ReleaseCi,
        "release.ci",
        "verify_release_ci",
        "核对精确双端源码的 CI 证据，或记录及复核同次全栈源码组合",
        CrossRepository,
        Contract,
        Backend,
        None,
        &[],
        &["显式指定的 CI 证据或源码组合收据"],
        &["GitHub API（仅 CI 证据模式）"]
    ),
];
