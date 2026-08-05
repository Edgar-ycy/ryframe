use std::{env, fmt, str::FromStr};

use ryframe_kernel::{AppError, AppResult};

/// 应用运行环境。
///
/// `APP_ENV` 只允许精确使用 `dev`、`test` 或 `prod`，不接受大小写、空白或别名。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Environment {
    /// 本地开发环境。
    #[default]
    Dev,
    /// 隔离运行环境。
    Test,
    /// 生产环境。
    Prod,
}

impl Environment {
    /// 普通进程读取 `APP_ENV`；未设置时使用开发环境。
    pub fn from_env() -> AppResult<Self> {
        match env::var("APP_ENV") {
            Ok(value) => value.parse(),
            Err(env::VarError::NotPresent) => Ok(Self::Dev),
            Err(env::VarError::NotUnicode(_)) => {
                Err(AppError::Config("APP_ENV 必须是有效的 UTF-8 字符串".into()))
            }
        }
    }

    /// 安全敏感进程读取 `APP_ENV`；变量必须显式设置。
    pub fn from_required_env() -> AppResult<Self> {
        match env::var("APP_ENV") {
            Ok(value) => value.parse(),
            Err(env::VarError::NotPresent) => Err(AppError::Config(
                "APP_ENV 必须显式设置为 dev、test 或 prod".into(),
            )),
            Err(env::VarError::NotUnicode(_)) => {
                Err(AppError::Config("APP_ENV 必须是有效的 UTF-8 字符串".into()))
            }
        }
    }

    /// 返回配置文件名和日志使用的稳定环境标识。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Test => "test",
            Self::Prod => "prod",
        }
    }

    /// 当前是否为生产环境。
    pub const fn is_production(self) -> bool {
        matches!(self, Self::Prod)
    }

    /// 当前是否为隔离运行环境。
    pub const fn is_test(self) -> bool {
        matches!(self, Self::Test)
    }
}

impl FromStr for Environment {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "dev" => Ok(Self::Dev),
            "test" => Ok(Self::Test),
            "prod" => Ok(Self::Prod),
            other => Err(AppError::Config(format!(
                "APP_ENV 必须精确设置为 dev、test 或 prod，当前值: {other}"
            ))),
        }
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
