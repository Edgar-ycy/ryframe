use std::collections::BTreeMap;

/// 在执行 DDL 前核对规范化描述的版本、目标和完整章节。
pub fn validate_canonical_schema(table: &str, value: &str) -> bool {
    let parts = split_unquoted(value, '|');
    let entries = sections(value);
    let list_keys = [
        "columns",
        "indexes",
        "constraints",
        "checks",
        "foreign_keys",
    ];
    parts.len() == 10
        && parts[0] == "v2"
        && entries.len() == 9
        && entries
            .get("table")
            .is_some_and(|name| *name == format!("{table:?}"))
        && ["engine", "charset", "collation"].iter().all(|key| {
            entries
                .get(key)
                .is_some_and(|value| value.starts_with('"') && value.ends_with('"'))
        })
        && list_keys.iter().all(|key| {
            entries
                .get(key)
                .is_some_and(|value| value.starts_with('[') && value.ends_with(']'))
        })
        && entries
            .get("columns")
            .is_some_and(|value| !rows(value).is_empty())
}

/// 对规范化结构给出可操作的差异，默认值等原始内容不进入诊断。
pub fn describe_schema_difference(expected: &str, actual: &str) -> String {
    if expected == actual {
        return "结构一致".into();
    }
    let left = sections(expected);
    let right = sections(actual);
    let mut differences = Vec::new();
    for (key, label) in [
        ("table", "表名"),
        ("engine", "存储引擎"),
        ("charset", "字符集"),
        ("collation", "排序规则"),
        ("columns", "列"),
        ("indexes", "索引"),
        ("constraints", "约束"),
        ("checks", "CHECK"),
        ("foreign_keys", "外键"),
    ] {
        if left.get(key) == right.get(key) {
            continue;
        }
        if key == "columns" {
            differences.extend(column_differences(
                left.get(key).copied().unwrap_or_default(),
                right.get(key).copied().unwrap_or_default(),
            ));
        } else if matches!(key, "indexes" | "constraints" | "checks" | "foreign_keys") {
            let before = rows(left.get(key).copied().unwrap_or_default());
            let after = rows(right.get(key).copied().unwrap_or_default());
            let changed = before
                .keys()
                .chain(after.keys())
                .filter(|name| before.get(*name) != after.get(*name))
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            differences.push(format!(
                "{label}变化：{}",
                changed.into_iter().collect::<Vec<_>>().join("、")
            ));
        } else {
            differences.push(format!("{label}不同"));
        }
    }
    if differences.is_empty() {
        "规范化格式或顺序不同".into()
    } else {
        differences.join("；")
    }
}

fn sections(value: &str) -> BTreeMap<&str, &str> {
    split_unquoted(value, '|')
        .into_iter()
        .filter_map(|part| part.split_once('='))
        .collect()
}

fn rows(value: &str) -> BTreeMap<String, Vec<&str>> {
    let mut result = BTreeMap::<String, Vec<&str>>::new();
    for row in split_unquoted(
        value
            .strip_prefix('[')
            .unwrap_or(value)
            .strip_suffix(']')
            .unwrap_or(value),
        ';',
    ) {
        if row.is_empty() {
            continue;
        }
        let key = split_unquoted(row, ':')[0].trim_matches('"').to_owned();
        result.entry(key).or_default().push(row);
    }
    result
}

fn column_differences(expected: &str, actual: &str) -> Vec<String> {
    let before = rows(expected);
    let after = rows(actual);
    let names = before
        .keys()
        .chain(after.keys())
        .collect::<std::collections::BTreeSet<_>>();
    let mut output = Vec::new();
    for name in names {
        match (before.get(name), after.get(name)) {
            (None, Some(_)) => output.push(format!("新增列 {name}")),
            (Some(_), None) => output.push(format!("缺少列 {name}")),
            (Some(left), Some(right)) if left != right => {
                let left = split_unquoted(left[0], ':');
                let right = split_unquoted(right[0], ':');
                let attributes = [
                    "列名",
                    "类型",
                    "可空性",
                    "字符集",
                    "排序规则",
                    "键标记",
                    "默认值",
                    "附加属性",
                    "生成表达式",
                ]
                .into_iter()
                .enumerate()
                .filter(|(index, _)| left.get(*index) != right.get(*index))
                .map(|(_, label)| label)
                .collect::<Vec<_>>();
                output.push(format!("列 {name} 的{}不同", attributes.join("、")));
            }
            _ => {}
        }
    }
    if output.is_empty() {
        output.push("列顺序不同".into());
    }
    output
}

pub(super) fn split_unquoted(value: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && character == '\\' {
            escaped = true;
            continue;
        }
        if character == '"' {
            quoted = !quoted;
        }
        if character == separator && !quoted {
            parts.push(&value[start..index]);
            start = index + character.len_utf8();
        }
    }
    parts.push(&value[start..]);
    parts
}
