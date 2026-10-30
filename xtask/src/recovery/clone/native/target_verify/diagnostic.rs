use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

struct Failure {
    cause: Box<dyn Error>,
    receipt: PathBuf,
    run_dir: PathBuf,
    reason: &'static str,
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "目标复核失败：阶段 verify_existing；原因：{}；原失败收据：{}；状态：未验收（clone_verified=false，restore_qualified=false）；使用 cargo xtask check recovery clone status --run-dir '{}' 核对阶段，再核对原失败收据及完整前后像；保留现场，不自动重放或清理",
            self.reason,
            self.receipt.display(),
            self.run_dir.display().to_string().replace('\'', "''"),
        )?;
        if self.reason == "目标 Redis namespace、owner 或 sentinel 与初始化不同"
            || self.reason == "目标当前 Redis 状态与初始化像不同"
        {
            write!(
                formatter,
                "；Redis 状态不再符合原初始化证明，保留原目标，后续只能按正式流程使用新的隔离目标继续"
            )?;
        }
        Ok(())
    }
}

impl fmt::Debug for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for Failure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

pub(super) fn saved(cause: Box<dyn Error>, receipt: &Path, run_dir: &Path) -> Box<dyn Error> {
    Box::new(Failure {
        reason: reason(cause.as_ref()),
        cause,
        receipt: receipt.to_owned(),
        run_dir: run_dir.to_owned(),
    })
}

pub(super) fn reason(error: &(dyn Error + 'static)) -> &'static str {
    let message = error.to_string();
    match message.as_str() {
        "输入路径不存在" => "输入路径不存在",
        "目标 Redis namespace、owner 或 sentinel 与初始化不同" => {
            "目标 Redis namespace、owner 或 sentinel 与初始化不同"
        }
        "目标当前 Redis 状态与初始化像不同" => "目标当前 Redis 状态与初始化像不同",
        "目标当前数据库库存与初始化像不同" => "目标当前数据库库存与初始化像不同",
        "两次完整库存不同，不能声明只读稳定" => {
            "两次完整库存不同，不能声明只读稳定"
        }
        "目标库存 CLI 执行失败，诊断已绑定" => "目标库存 CLI 执行失败，诊断已绑定",
        "数据库固定只读核验失败，安全诊断已绑定" => {
            "数据库固定只读核验失败，安全诊断已绑定"
        }
        "采集期间来源、工具、配置或物理绑定变化" => {
            "采集期间来源、工具、配置或物理绑定变化"
        }
        "调用库存 CLI 前来源或配置已变化" => "调用库存 CLI 前来源或配置已变化",
        "四目标库存来自不同编译 schema" => "四目标库存来自不同编译 schema",
        message if message == super::database::model::TABLE_SET_ERROR => {
            "库存完整表集、保留表或迁移账本不同"
        }
        "目标当前对象状态与初始化像不同" => "目标当前对象状态与初始化像不同",
        "初始库存原始文件或摘要变化" => "初始库存原始文件或摘要变化",
        "目标初始化历史文件集合或摘要变化" => "目标初始化历史文件集合或摘要变化",
        "目标 reset 完成证据与初始化代次不同" => {
            "目标 reset 完成证据与初始化代次不同"
        }
        "目标 generation、请求或 scope 绑定不同" => {
            "目标 generation、请求或 scope 绑定不同"
        }
        "目标维护构建代次已变化" => "目标维护构建代次已变化",
        "库存采集后的完整证据集合发生变化" => "库存采集后的完整证据集合发生变化",
        "目标 initialized 发布绑定在核验期间变化" => {
            "目标 initialized 发布绑定在核验期间变化"
        }
        "目标完整文件树在核验期间变化" => "目标完整文件树在核验期间变化",
        "执行源码在核验期间变化" => "执行源码在核验期间变化",
        "目标锁释放后只读权威与初像不同" => "目标锁释放后只读权威与初像不同",
        _ => object_reason(&message).unwrap_or("未分类的目标复核错误（TargetVerifyError）"),
    }
}

fn object_reason(message: &str) -> Option<&'static str> {
    for bucket in super::super::input::BUCKETS {
        for suffix in [
            "对象 key 集合与初始化不同",
            "owner 字节或摘要与初始化不同",
            "owner 字节不符",
            "启用了版本化，不能作为 fresh 目标",
        ] {
            if message == format!("目标 {bucket} {suffix}") {
                return Some("目标对象 key、owner 或版本状态与初始化要求不同");
            }
        }
    }
    None
}

pub(crate) fn diagnostic(error: &(dyn Error + 'static)) -> Option<(String, String)> {
    if let Some(failure) = error.downcast_ref::<Failure>() {
        return Some(("TargetVerifyError".into(), failure.to_string()));
    }
    error.source().and_then(diagnostic)
}
