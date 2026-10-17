use std::{collections::BTreeMap, path::PathBuf};

use serde::Serialize;

use crate::Result;

const PERCENT_SCALE: u128 = 100_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Percent(u32);

impl Percent {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        if whole.is_empty()
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
            || fraction.len() > 6
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
            || (value.contains('.') && fraction.is_empty())
        {
            return Err("百分比必须为 0 到 100 的十进制数，最多六位小数".into());
        }
        let whole: u32 = whole.parse().map_err(|_| "百分比整数过大")?;
        let fraction: u32 = format!("{fraction:0<6}").parse()?;
        let units = whole
            .checked_mul(1_000_000)
            .and_then(|value| value.checked_add(fraction))
            .filter(|value| u128::from(*value) <= PERCENT_SCALE)
            .ok_or("百分比必须为 0 到 100")?;
        Ok(Self(units))
    }

    pub(crate) fn text(self) -> String {
        format!("{}.{:06}", self.0 / 1_000_000, self.0 % 1_000_000)
    }

    pub(crate) fn accepts(self, rust: u64, code: u64) -> bool {
        code != 0 && u128::from(rust) * PERCENT_SCALE >= u128::from(code) * u128::from(self.0)
    }
}

pub(crate) fn ratio_text(rust: u64, code: u64) -> Option<String> {
    if code == 0 {
        return None;
    }
    let units = u128::from(rust) * PERCENT_SCALE / u128::from(code);
    Some(format!("{}.{:06}", units / 1_000_000, units % 1_000_000))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LanguageOptions {
    pub(crate) min_rust_percent: Option<Percent>,
    pub(crate) output: Option<PathBuf>,
    pub(crate) write: bool,
    pub(crate) plan: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Code,
    Document,
    Config,
    Resource,
}

impl Kind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Code => "代码",
            Self::Document => "文档",
            Self::Config => "配置",
            Self::Resource => "资源",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FileRecord {
    pub(crate) path: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
    pub(crate) kind: Kind,
    pub(crate) language: Option<String>,
    pub(crate) vendor: bool,
    pub(crate) generated: bool,
    pub(crate) test: bool,
    pub(crate) template: bool,
    pub(crate) fallback: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Count {
    pub(crate) files: u64,
    pub(crate) bytes: u64,
}

impl Count {
    pub(crate) fn add(&mut self, bytes: u64) -> Result<()> {
        self.files = self.files.checked_add(1).ok_or("文件数量溢出")?;
        self.bytes = self.bytes.checked_add(bytes).ok_or("文件字节总量溢出")?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Summary {
    pub(crate) total: Count,
    pub(crate) code: Count,
    pub(crate) rust: Count,
    pub(crate) categories: BTreeMap<String, Count>,
    pub(crate) languages: BTreeMap<String, Count>,
    pub(crate) extensions: BTreeMap<String, Count>,
    pub(crate) code_groups: BTreeMap<String, Count>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub(crate) format_version: u32,
    pub(crate) policy_sha256: String,
    pub(crate) policy: super::policy::Policy,
    pub(crate) head: Option<String>,
    pub(crate) source_sha256: String,
    pub(crate) total: Count,
    pub(crate) self_maintained: Summary,
    pub(crate) vendor: Summary,
    pub(crate) rust_percent: Option<String>,
    pub(crate) min_rust_percent: Option<String>,
    pub(crate) threshold_passed: Option<bool>,
    pub(crate) ignored_tracked: Vec<String>,
    pub(crate) deleted_tracked: Vec<String>,
    pub(crate) files: Vec<FileRecord>,
}
