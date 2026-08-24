use std::collections::HashSet;

use crate::entities::{dict_data, dict_type};

/// 已软删除的字典类型不得因其复合外键使初始化失败，也不得使数据在未复制类型的
/// 情况下仍可访问。
pub(super) fn retain_data_for_active_dict_types(
    dictionary_types: &[dict_type::Model],
    dictionary_data: &mut Vec<dict_data::Model>,
) {
    let active_codes: HashSet<&str> = dictionary_types
        .iter()
        .map(|dictionary| dictionary.code.as_str())
        .collect();
    dictionary_data.retain(|data| active_codes.contains(data.type_code.as_str()));
}
