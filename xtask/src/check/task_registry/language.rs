use super::{TaskDefinition, TaskExecutor, TaskRepository, TaskStage, TaskWorkingDirectory};

pub(super) const TASKS: [TaskDefinition; 3] = [
    task_definition!(
        LanguageReport,
        "language.report",
        "report_language_bytes",
        "只读发现 Git 非 ignored 文件并按固定字节口径输出语言报告",
        Backend,
        Static,
        Backend,
        None,
        &[],
        &[],
        &[]
    ),
    task_definition!(
        LanguageWrite,
        "language.write",
        "write_language_report",
        "仅以 --output 和 --write 在忽略目录创建新报告，拒绝覆盖",
        Backend,
        Static,
        Backend,
        None,
        &[],
        &["显式指定的 .local-tests JSON 新报告"],
        &[]
    ),
    task_definition!(
        LanguageThreshold,
        "language.threshold",
        "check_language_threshold",
        "以整数交叉相乘检查显式 Rust 阈值，不将空分母或未达标判为通过",
        Backend,
        Static,
        Backend,
        None,
        &[],
        &[],
        &[]
    ),
];
