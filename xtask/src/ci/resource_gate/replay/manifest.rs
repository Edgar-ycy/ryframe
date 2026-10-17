use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value};

pub(crate) const FORMAT_VERSION: u64 = 2;
pub(crate) const MINIMUM_CASES: usize = 20;
pub(crate) const MINIMUM_SUCCESSFUL_CASES: usize = 10;
const REQUIRED_CATEGORIES: [&str; 7] = [
    "addition",
    "field",
    "permission",
    "relation",
    "sql",
    "rename",
    "delete",
];
const TARGETED_MODES: [&str; 2] = ["targeted", "full"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplayCase {
    pub(crate) name: String,
    pub(crate) category: String,
    pub(crate) base: String,
    pub(crate) head: String,
    pub(crate) frontend_base: String,
    pub(crate) frontend_head: String,
    pub(crate) targeted_mode: String,
    pub(crate) expected: String,
}

impl ReplayCase {
    pub(crate) fn expected_pass(&self) -> bool {
        self.expected == "pass"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplayManifest {
    pub(crate) targeted_command: Vec<String>,
    pub(crate) full_command: Vec<String>,
    pub(crate) cases: Vec<ReplayCase>,
}

pub(crate) fn load_manifest(path: &Path) -> Result<ReplayManifest, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("无法读取 replay manifest {}：{error}", path.display()))?;
    let raw = serde_json::from_str::<Value>(&source)
        .map_err(|error| format!("无法读取 replay manifest：{error}"))?;
    let object = raw
        .as_object()
        .ok_or_else(|| "replay manifest 必须是 JSON 对象".to_owned())?;
    require_exact_keys(
        object,
        &["formatVersion", "targetedCommand", "fullCommand", "cases"],
        "manifest",
    )?;
    let version = object
        .get("formatVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| "replay manifest.formatVersion 必须是整数".to_owned())?;
    if version != FORMAT_VERSION {
        return Err(format!(
            "replay manifest formatVersion 必须为 {FORMAT_VERSION}"
        ));
    }
    let targeted_command = parse_command(
        object
            .get("targetedCommand")
            .ok_or_else(|| "缺少 targetedCommand".to_owned())?,
        "targetedCommand",
    )?;
    let full_command = parse_command(
        object
            .get("fullCommand")
            .ok_or_else(|| "缺少 fullCommand".to_owned())?,
        "fullCommand",
    )?;
    let cases = object
        .get("cases")
        .and_then(Value::as_array)
        .ok_or_else(|| "replay manifest.cases 必须是数组".to_owned())?
        .iter()
        .enumerate()
        .map(|(index, value)| parse_case(value, index))
        .collect::<Result<Vec<_>, _>>()?;
    validate_case_coverage(&cases)?;
    Ok(ReplayManifest {
        targeted_command,
        full_command,
        cases,
    })
}

fn require_exact_keys(
    object: &Map<String, Value>,
    expected: &[&str],
    label: &str,
) -> Result<(), String> {
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if actual != expected {
        let missing = expected.difference(&actual).copied().collect::<Vec<_>>();
        let extra = actual.difference(&expected).copied().collect::<Vec<_>>();
        return Err(format!(
            "{label} 字段不匹配，缺少={missing:?}，多余={extra:?}"
        ));
    }
    Ok(())
}

fn parse_command(value: &Value, label: &str) -> Result<Vec<String>, String> {
    let items = value
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or_else(|| format!("{label} 必须是非空字符串参数数组"))?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("{label} 必须是非空字符串参数数组"))
        })
        .collect()
}

fn parse_case(value: &Value, index: usize) -> Result<ReplayCase, String> {
    let label = format!("cases[{index}]");
    let object = value
        .as_object()
        .ok_or_else(|| format!("{label} 必须是对象"))?;
    require_exact_keys(
        object,
        &[
            "name",
            "category",
            "base",
            "head",
            "frontendBase",
            "frontendHead",
            "targetedMode",
            "expected",
        ],
        &label,
    )?;
    let string = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("{label}.{key} 必须是非空字符串"))
    };
    let trimmed = |key: &str| -> Result<String, String> {
        let value = string(key)?;
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("{label}.{key} 必须是非空字符串"));
        }
        Ok(value.to_owned())
    };
    let commit = |key: &str| {
        let value = object
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{label}.{key} 必须是 40 位 Git SHA"))?;
        if !valid_sha(value) {
            return Err(format!("{label}.{key} 必须是 40 位 Git SHA"));
        }
        Ok(value.to_ascii_lowercase())
    };
    let targeted_mode = string("targetedMode")?;
    if !TARGETED_MODES.contains(&targeted_mode.as_str()) {
        return Err(format!("{label}.targetedMode 只允许 targeted 或 full"));
    }
    let expected = string("expected")?;
    if expected != "pass" && expected != "fail" {
        return Err(format!("{label}.expected 只允许 pass 或 fail"));
    }
    Ok(ReplayCase {
        name: trimmed("name")?,
        category: trimmed("category")?,
        base: commit("base")?,
        head: commit("head")?,
        frontend_base: commit("frontendBase")?,
        frontend_head: commit("frontendHead")?,
        targeted_mode,
        expected,
    })
}

fn valid_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn validate_case_coverage(cases: &[ReplayCase]) -> Result<(), String> {
    if cases.len() < MINIMUM_CASES {
        return Err(format!("replay 至少需要 {MINIMUM_CASES} 个案例"));
    }
    let names = cases
        .iter()
        .map(|case| case.name.as_str())
        .collect::<BTreeSet<_>>();
    if names.len() != cases.len() {
        return Err("replay 案例名称必须唯一".to_owned());
    }
    let ranges = cases
        .iter()
        .map(|case| (case.base.as_str(), case.head.as_str()))
        .collect::<BTreeSet<_>>();
    if ranges.len() != cases.len() {
        return Err("replay 案例的 base/head 范围必须唯一".to_owned());
    }
    let categories = cases
        .iter()
        .map(|case| case.category.as_str())
        .collect::<BTreeSet<_>>();
    let missing = REQUIRED_CATEGORIES
        .into_iter()
        .filter(|category| !categories.contains(category))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!("replay 缺少类别：{}", missing.join(", ")));
    }
    let unchanged = cases
        .iter()
        .filter(|case| case.base == case.head)
        .map(|case| case.name.as_str())
        .collect::<Vec<_>>();
    if !unchanged.is_empty() {
        return Err(format!(
            "replay 案例 base/head 不得相同：{}",
            unchanged.join(", ")
        ));
    }
    let outcomes = cases
        .iter()
        .map(|case| case.expected.as_str())
        .collect::<BTreeSet<_>>();
    if outcomes != BTreeSet::from(["fail", "pass"]) {
        return Err("replay 必须同时覆盖预期 pass 与 fail 案例".to_owned());
    }
    let modes = cases
        .iter()
        .map(|case| case.targeted_mode.as_str())
        .collect::<BTreeSet<_>>();
    if modes != BTreeSet::from(["full", "targeted"]) {
        return Err("replay 必须同时覆盖 targeted 与 full 模式".to_owned());
    }
    let targeted_passes = cases
        .iter()
        .filter(|case| case.expected == "pass" && case.targeted_mode == "targeted")
        .count();
    if targeted_passes < MINIMUM_SUCCESSFUL_CASES {
        return Err(format!(
            "replay 至少需要 {MINIMUM_SUCCESSFUL_CASES} 个预期成功的 targeted 案例"
        ));
    }
    Ok(())
}

pub(crate) fn report_allowed_root(repository: &Path) -> PathBuf {
    repository.join(".local-tests").join("resource-gate-replay")
}
