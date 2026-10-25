use std::{error::Error, path::Path};

use ryframe_config::{AppConfig, Environment, TenantDatabaseTargetKind};

pub(super) type Failure = Box<dyn Error>;

#[derive(Clone, Copy)]
pub(super) enum Writes {
    None,
    Database,
    File,
}

#[derive(Clone, Copy)]
pub(super) struct Context<'a> {
    pub directory: &'a Path,
    pub environment: Option<Environment>,
    pub config: Option<&'a AppConfig>,
    pub command: &'a str,
    pub target: Option<&'a str>,
}

impl Context<'_> {
    pub fn error(&self, phase: &str, writes: Writes, error: &(dyn Error + 'static)) -> Failure {
        let cause = self.cause(error);
        let files = match self.environment {
            Some(environment) => format!(
                "{}（可选）与 {}",
                self.directory.join("app.toml").display(),
                self.directory
                    .join(format!("app.{environment}.toml"))
                    .display()
            ),
            None => "尚未选择或读取运行配置".into(),
        };
        let state = match writes {
            Writes::None => "当前命令尚未发起迁移、接管账本或导出文件写入；此前命令可能已完成 DDL",
            Writes::Database => {
                "已进入写入阶段，DDL 或账本可能已部分提交；不代表整体回滚，保留现场核对，禁止自动重放"
            }
            Writes::File => {
                "已进入文件准备或发布阶段，目录、临时文件或目标文件可能已创建；保留现场核对，禁止覆盖或自动重新导出"
            }
        };
        let field = self.field();
        let target = self.target.filter(|value| safe_name(value)).unwrap_or(
            if self.command.starts_with("tenant-data") {
                "全部所选租户目标"
            } else {
                "control"
            },
        );
        let message = format!(
            "迁移命令失败：操作={}；目标={target}；阶段={phase}；配置文件={files}；配置键={field}；原因链={cause}；{state}。{} 命令中的 <当前已显示配置目录> 必须替换为上述配置文件的父目录并按当前 shell 正确引用；PowerShell 使用单引号时，目录内单引号写为两个单引号。目录尚未确定时先修正参数，不执行占位命令。",
            self.command,
            self.repair()
        );
        self.redact(&message).into()
    }

    fn cause(&self, error: &(dyn Error + 'static)) -> String {
        let mut messages = vec![self.redact(&error.to_string())];
        let mut next = error.source();
        while let Some(source) = next {
            let message = self.redact(&source.to_string());
            if !messages.contains(&message) {
                messages.push(message);
            }
            next = source.source();
        }
        messages.join(" -> ")
    }

    fn redact(&self, message: &str) -> String {
        self.config.map_or_else(
            || message.to_owned(),
            |config| ryframe::database_diagnostic::redact(config, message),
        )
    }

    fn field(&self) -> String {
        if self.command == "control export-sql" {
            return "bootstrap.admin_password / --output".into();
        }
        let Some(target) = self.target else {
            return if self.command.starts_with("tenant-data") {
                "tenant_data.targets".into()
            } else {
                "database.primary".into()
            };
        };
        if let Some((index, value)) = self.config.and_then(|config| {
            config
                .tenant_data
                .targets
                .iter()
                .enumerate()
                .find(|(_, item)| item.key == target)
        }) {
            return if value.kind == TenantDatabaseTargetKind::Control {
                format!("tenant_data.targets[{index}] / database.primary")
            } else {
                format!("tenant_data.targets[{index}]")
            };
        }
        if target == "shared-control" {
            "database.primary / tenant_data.targets".into()
        } else {
            "tenant_data.targets[].key".into()
        }
    }

    fn repair(&self) -> String {
        if self.config.is_none() {
            return "请先按当前阶段修正参数、选定文件及字段；配置加载完成前不连接数据库。修正后使用同一配置先执行 cargo migrate -- --config-dir <当前已显示配置目录> control status 或 cargo migrate -- --config-dir <当前已显示配置目录> tenant-data status --target <键>；不自动重试写入。".into();
        }
        if self.command == "control export-sql" {
            return "请核对选定配置的 bootstrap.admin_password、输出路径权限和文件存在性；先审阅 cargo migrate -- --config-dir <当前已显示配置目录> control export-sql --output <新路径> --plan，不覆盖原文件。".into();
        }
        let adoption = if self.command.contains("adopt") {
            " 若已生成原接管计划，另用原范围执行 cargo migrate -- --config-dir <当前已显示配置目录> control adopt --inspect <原计划> 或 cargo migrate -- --config-dir <当前已显示配置目录> tenant-data adopt --inspect <原计划> --target <键>，不能重新生成计划掩盖未知提交。"
        } else {
            ""
        };
        let commands = self.target.filter(|value| safe_name(value)).map_or_else(
            || if self.command.starts_with("tenant-data") {
                "cargo migrate -- --config-dir <当前已显示配置目录> tenant-data status --all；cargo migrate -- --config-dir <当前已显示配置目录> tenant-data verify --all".to_owned()
            } else { "cargo migrate -- --config-dir <当前已显示配置目录> control status；cargo migrate -- --config-dir <当前已显示配置目录> control verify".to_owned() },
            |target| format!("cargo migrate -- --config-dir <当前已显示配置目录> tenant-data status --target {target}；cargo migrate -- --config-dir <当前已显示配置目录> tenant-data verify --target {target}"));
        format!(
            "请核对选定配置中的 host、port、database、账号连接与数据库访问权限、tls_mode、tls_ca、tls_client_cert、tls_client_key 及连接超时/池容量；不可用不等于认证失败，独立 MySQL 目标的 registry 可能未保留底层原因。修正后使用同一 --config-dir 和 APP_ENV 先执行只读核对：{commands}；up 或 adopt --apply 只能在核对完整现场后显式决定，不能自动重试。{adoption}"
        )
    }
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

/// 操作失败优先，关闭失败仍保留；不为恢复诊断重新执行任何操作。
pub(super) fn finish<T>(
    result: Result<T, Failure>,
    close: Result<(), Failure>,
) -> Result<T, Failure> {
    match (result, close) {
        (Err(error), Err(close)) => Err(format!("{error}；关闭目标连接时另有错误：{close}").into()),
        (Err(error), _) => Err(error),
        (_, Err(close)) => Err(close),
        (Ok(value), Ok(())) => Ok(value),
    }
}
