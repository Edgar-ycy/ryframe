//! 数据变更比对（Data Diff）
//!
//! 用于操作日志中记录字段级变更前后差异。
//! 支持 JSON 对象、HashMap 等多种输入格式。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 单个字段的变更记录
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldChange {
    /// 字段名
    pub field: String,
    /// 变更前值（None 表示新增）
    pub old_value: Option<String>,
    /// 变更后值（None 表示删除）
    pub new_value: Option<String>,
}

/// 数据变更差异
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DataDiff {
    /// 变更的字段列表
    pub changes: Vec<FieldChange>,
    /// 变更字段数
    pub changed_count: usize,
    /// 是否有变更
    pub has_changes: bool,
}

impl DataDiff {
    /// 创建空 diff
    pub fn new() -> Self {
        Self {
            changes: Vec::new(),
            changed_count: 0,
            has_changes: false,
        }
    }

    /// 从两个 JSON Value 计算差异
    ///
    /// 仅比较第一层字段（shallow diff），适用于扁平化的业务对象。
    pub fn from_json(old: &Value, new: &Value) -> Self {
        let mut changes = Vec::new();

        if let (Value::Object(old_map), Value::Object(new_map)) = (old, new) {
            // 收集所有键（去重）
            let mut all_keys: Vec<&String> = old_map.keys().collect();
            for k in new_map.keys() {
                if !all_keys.contains(&k) {
                    all_keys.push(k);
                }
            }

            for key in all_keys {
                let old_val = old_map.get(key);
                let new_val = new_map.get(key);

                let old_str = old_val.map(value_to_string);
                let new_str = new_val.map(value_to_string);

                if old_str != new_str {
                    changes.push(FieldChange {
                        field: key.clone(),
                        old_value: old_str,
                        new_value: new_str,
                    });
                }
            }
        }

        let changed_count = changes.len();
        DataDiff {
            has_changes: changed_count > 0,
            changes,
            changed_count,
        }
    }

    /// 从两个 HashMap 计算差异
    pub fn from_maps(old: &HashMap<String, String>, new: &HashMap<String, String>) -> Self {
        let mut changes = Vec::new();

        let mut all_keys: Vec<&String> = old.keys().collect();
        for k in new.keys() {
            if !all_keys.contains(&k) {
                all_keys.push(k);
            }
        }

        for key in all_keys {
            let old_val = old.get(key);
            let new_val = new.get(key);

            if old_val != new_val {
                changes.push(FieldChange {
                    field: key.clone(),
                    old_value: old_val.cloned(),
                    new_value: new_val.cloned(),
                });
            }
        }

        let changed_count = changes.len();
        DataDiff {
            has_changes: changed_count > 0,
            changes,
            changed_count,
        }
    }

    /// 格式化为可读的文本
    ///
    /// # Example
    ///
    /// ```
    /// use ryframe_common::utils::data_diff::DataDiff;
    ///
    /// let diff = DataDiff {
    ///     changes: vec![
    ///         ryframe_common::utils::data_diff::FieldChange {
    ///             field: "name".into(),
    ///             old_value: Some("张三".into()),
    ///             new_value: Some("李四".into()),
    ///         },
    ///     ],
    ///     changed_count: 1,
    ///     has_changes: true,
    /// };
    /// assert!(diff.format_text().contains("张三 → 李四"));
    /// ```
    pub fn format_text(&self) -> String {
        if !self.has_changes {
            return "无变更".to_string();
        }

        self.changes
            .iter()
            .map(|c| {
                format!(
                    "[{}] {} → {}",
                    c.field,
                    c.old_value.as_deref().unwrap_or("(空)"),
                    c.new_value.as_deref().unwrap_or("(空)")
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// 序列化为 JSON 字符串（用于存入数据库）
    pub fn to_json_string(&self) -> String {
        serde_json::to_string(&self.changes).unwrap_or_else(|_| "[]".to_string())
    }

    /// 从 JSON 字符串解析
    pub fn from_json_string(s: &str) -> Self {
        let changes: Vec<FieldChange> = serde_json::from_str(s).unwrap_or_default();
        let changed_count = changes.len();
        Self {
            has_changes: changed_count > 0,
            changes,
            changed_count,
        }
    }
}

impl Default for DataDiff {
    fn default() -> Self {
        Self::new()
    }
}

/// 宽松的 JSON 值 → 字符串转换
fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        // 对数组/对象做紧凑 JSON 序列化
        _ => serde_json::to_string(v).unwrap_or_else(|_| format!("{:?}", v)),
    }
}

/// DataDiff 辅助构造器
pub struct DataDiffBuilder {
    old_fields: HashMap<String, String>,
    new_fields: HashMap<String, String>,
}

impl DataDiffBuilder {
    /// 创建新的 diff 构造器
    pub fn new() -> Self {
        Self {
            old_fields: HashMap::new(),
            new_fields: HashMap::new(),
        }
    }

    /// 记录变更前字段值
    pub fn old_val(mut self, field: &str, value: impl ToString) -> Self {
        self.old_fields.insert(field.to_string(), value.to_string());
        self
    }

    /// 记录变更后字段值
    pub fn new_val(mut self, field: &str, value: impl ToString) -> Self {
        self.new_fields.insert(field.to_string(), value.to_string());
        self
    }

    /// 同时记录变更前后
    pub fn change(self, field: &str, old_value: impl ToString, new_value: impl ToString) -> Self {
        self.old_val(field, old_value).new_val(field, new_value)
    }

    /// 构建 DataDiff
    pub fn build(&self) -> DataDiff {
        DataDiff::from_maps(&self.old_fields, &self.new_fields)
    }
}

impl Default for DataDiffBuilder {
    fn default() -> Self {
        Self::new()
    }
}
