use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 只读导入保留的引用动作；序列化与 MySQL canonical schema 使用相同的 SQL 名称。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ForeignKeyAction {
    Cascade,
    Restrict,
    #[serde(rename = "SET NULL")]
    SetNull,
    #[serde(rename = "NO ACTION")]
    NoAction,
    #[serde(rename = "SET DEFAULT")]
    SetDefault,
}

impl FromStr for ForeignKeyAction {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_uppercase().as_str() {
            "CASCADE" => Ok(Self::Cascade),
            "RESTRICT" => Ok(Self::Restrict),
            "SET NULL" => Ok(Self::SetNull),
            "NO ACTION" => Ok(Self::NoAction),
            "SET DEFAULT" => Ok(Self::SetDefault),
            _ => Err(format!("未知外键引用动作 {value:?}，拒绝丢失结构语义")),
        }
    }
}

/// 保留数据库报告的匹配规则，不根据 CREATE TABLE 中被数据库忽略的声明推断。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ForeignKeyMatch {
    None,
    Simple,
    Full,
    Partial,
}

impl FromStr for ForeignKeyMatch {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_uppercase().as_str() {
            "NONE" => Ok(Self::None),
            "SIMPLE" => Ok(Self::Simple),
            "FULL" => Ok(Self::Full),
            "PARTIAL" => Ok(Self::Partial),
            _ => Err(format!("未知外键匹配规则 {value:?}，拒绝丢失结构语义")),
        }
    }
}
