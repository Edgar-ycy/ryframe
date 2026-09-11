use std::collections::BTreeSet;

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskRepository {
    Backend,
    Frontend,
    CrossRepository,
}

impl TaskRepository {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Backend => "后端",
            Self::Frontend => "前端",
            Self::CrossRepository => "跨仓",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskStage {
    Prerequisite,
    Static,
    Test,
    Contract,
    Build,
}

impl TaskStage {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Prerequisite => "前置",
            Self::Static => "静态",
            Self::Test => "测试",
            Self::Contract => "契约",
            Self::Build => "构建",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskWorkingDirectory {
    Backend,
    Frontend,
}

impl TaskWorkingDirectory {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Backend => "后端工作区",
            Self::Frontend => "前端工作区",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskSpec<Executor> {
    pub(crate) id: &'static str,
    pub(crate) dependencies: Vec<&'static str>,
    pub(crate) repository: TaskRepository,
    pub(crate) stage: TaskStage,
    pub(crate) working_directory: TaskWorkingDirectory,
    pub(crate) executor: Executor,
    pub(crate) compilation_coverage: &'static [&'static str],
    pub(crate) allowed_writes: &'static [&'static str],
    pub(crate) external_resources: &'static [&'static str],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskPlan<Executor> {
    pub(crate) tasks: Vec<TaskSpec<Executor>>,
}

impl<Executor> TaskPlan<Executor> {
    pub(crate) fn new(tasks: Vec<TaskSpec<Executor>>) -> Result<Self> {
        validate_task_graph(&tasks)?;
        Ok(Self { tasks })
    }

    pub(crate) fn validate(&self) -> Result<()> {
        validate_task_graph(&self.tasks)
    }
}

impl<Executor: PartialEq> TaskPlan<Executor> {
    pub(crate) fn contains(&self, executor: &Executor) -> bool {
        self.tasks.iter().any(|task| &task.executor == executor)
    }
}

fn validate_task_graph<Executor>(tasks: &[TaskSpec<Executor>]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for task in tasks {
        if !seen.insert(task.id) {
            return Err(format!("检查任务 ID 重复：{}", task.id).into());
        }
        for dependency in &task.dependencies {
            if !seen.contains(dependency) {
                return Err(format!(
                    "检查任务 {} 的依赖 {} 不存在或未按拓扑顺序声明",
                    task.id, dependency
                )
                .into());
            }
        }
    }
    Ok(())
}
