use serde::Serialize;

/// 受支持的 MySQL 物理引用动作，不表示应用级联操作。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResourceForeignKeyAction {
    Cascade,
    Restrict,
    #[serde(rename = "SET NULL")]
    SetNull,
    #[serde(rename = "NO ACTION")]
    NoAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResourceForeignKeyMatch {
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ResourceForeignKeyDescriptor {
    pub name: &'static str,
    pub columns: &'static [&'static str],
    pub referenced_table: &'static str,
    pub referenced_columns: &'static [&'static str],
    pub update_rule: ResourceForeignKeyAction,
    pub delete_rule: ResourceForeignKeyAction,
    pub match_option: ResourceForeignKeyMatch,
}
