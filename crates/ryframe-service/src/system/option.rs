use serde::Serialize;

/// 选择器中的单个候选项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OptionItem {
    pub value: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub disabled: bool,
}

/// 不依赖总数查询的有界选择器结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OptionList {
    pub items: Vec<OptionItem>,
    pub has_more: bool,
}
