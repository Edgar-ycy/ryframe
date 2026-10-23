use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use chrono::{Duration, Local, NaiveDate};
use serde::Deserialize;
use serde_json::json;
use toml::{Table, Value};

use super::support::{command_stdout, python_executable};
#[path = "ast/changed.rs"]
pub(crate) mod changed;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FunctionRecord {
    pub(crate) path: String,
    pub(crate) symbol: String,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) lines: usize,
    pub(crate) private: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct UnsafeSyntaxRecord {
    path: String,
    line: usize,
    context: String,
}

#[derive(Debug, Default, Deserialize)]
struct AstResponse {
    functions: Vec<FunctionRecord>,
    unsafe_syntax: Vec<UnsafeSyntaxRecord>,
    errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FunctionRule {
    path: String,
    symbol: String,
}

#[derive(Debug, Clone)]
struct FunctionException {
    rule: FunctionRule,
    current_lines: usize,
}

#[derive(Debug)]
pub(crate) struct FunctionPolicy {
    mode: String,
    public_max_lines: usize,
    private_max_lines: usize,
    orchestration_max_lines: usize,
    orchestrations: BTreeSet<FunctionRule>,
    exceptions: Vec<FunctionException>,
}

#[derive(Debug, Default)]
pub(crate) struct AstStats {
    pub(crate) checked_unsafe_sources: usize,
    pub(crate) unsafe_violations: usize,
    pub(crate) checked_functions: usize,
    pub(crate) function_violations: usize,
    pub(crate) excluded_frozen_functions: usize,
}

pub(crate) fn validate_ast(
    root: &Path,
    sources: &[String],
    templates: &[String],
    function_size: &Table,
    frozen_migrations: &BTreeSet<String>,
    base_sha: Option<&str>,
    errors: &mut Vec<String>,
) -> AstStats {
    let response = run_helper(root, sources, templates, errors);
    let checked_unsafe_sources = sources.len() + templates.len();
    let unsafe_violations = validate_no_unsafe(&response.unsafe_syntax, errors);
    let Some(policy) = parse_policy(function_size, errors) else {
        return AstStats {
            checked_unsafe_sources,
            unsafe_violations,
            ..AstStats::default()
        };
    };
    let retained = policy
        .exceptions
        .iter()
        .map(|exception| (exception.rule.path.clone(), exception.rule.symbol.clone()))
        .collect::<BTreeSet<_>>();
    let mutable = mutable_function_records(&response.functions, frozen_migrations, &retained);
    let excluded_frozen_functions = response.functions.len() - mutable.len();
    let changed = if policy.mode == "changed" {
        changed::changed_line_ranges(root, base_sha, errors)
    } else {
        None
    };
    let (checked_functions, function_violations) =
        validate_functions(&policy, &mutable, changed.as_ref(), errors);
    AstStats {
        checked_unsafe_sources,
        unsafe_violations,
        checked_functions,
        function_violations,
        excluded_frozen_functions,
    }
}

fn run_helper(
    root: &Path,
    sources: &[String],
    templates: &[String],
    errors: &mut Vec<String>,
) -> AstResponse {
    let request = json!({
        "root": root.to_string_lossy(),
        "sources": sources,
        "templates": templates,
    })
    .to_string();
    let output = match python_executable(root).and_then(|program| {
        command_stdout(
            root,
            program.as_os_str(),
            &["tools/python/rust_function_size.py", "--ast-json"],
            Some(&request),
        )
    }) {
        Ok(output) => output,
        Err(error) => {
            errors.push(format!("Rust AST helper 执行失败: {error}"));
            return AstResponse::default();
        }
    };
    match serde_json::from_str::<AstResponse>(&output) {
        Ok(response) => {
            errors.extend(response.errors.iter().cloned());
            response
        }
        Err(error) => {
            errors.push(format!("Rust AST helper 输出不是有效 JSON: {error}"));
            AstResponse::default()
        }
    }
}

fn validate_no_unsafe(records: &[UnsafeSyntaxRecord], errors: &mut Vec<String>) -> usize {
    for record in records {
        errors.push(format!(
            "自维护 Rust 源码禁止 unsafe 语法：{}:{}（{}）",
            record.path, record.line, record.context
        ));
    }
    records.len()
}

pub(crate) fn parse_policy(raw: &Table, errors: &mut Vec<String>) -> Option<FunctionPolicy> {
    let mode = match raw.get("mode").and_then(Value::as_str) {
        Some(mode @ ("changed" | "all")) => mode.to_owned(),
        _ => {
            errors.push("function_size.mode 只允许 changed 或 all".into());
            "changed".into()
        }
    };
    let public_max_lines = positive(raw, "public_max_lines", errors)?;
    let private_max_lines = positive(raw, "private_max_lines", errors)?;
    let orchestration_max_lines = positive(raw, "orchestration_max_lines", errors)?;
    let max_exception_days = positive(raw, "max_exception_days", errors)?;
    if orchestration_max_lines > private_max_lines || private_max_lines > public_max_lines {
        errors.push("函数长度上限必须满足 orchestration <= private <= public".into());
    }
    let orchestrations = parse_rules(
        raw.get("orchestrations"),
        "function_size.orchestrations",
        errors,
    );
    let exceptions = parse_exceptions(raw.get("exceptions"), max_exception_days, errors);
    let duplicate_exceptions = duplicate_rules(
        &exceptions
            .iter()
            .map(|exception| exception.rule.clone())
            .collect::<Vec<_>>(),
    );
    if !duplicate_exceptions.is_empty() {
        errors.push(format!(
            "function_size.exceptions 不得重复：{}",
            format_rules(&duplicate_exceptions)
        ));
    }
    if mode == "all" && !exceptions.is_empty() {
        errors.push("function_size.mode 只能在 exceptions 清零后切换为 all".into());
    }
    Some(FunctionPolicy {
        mode,
        public_max_lines,
        private_max_lines,
        orchestration_max_lines,
        orchestrations: orchestrations.into_iter().collect(),
        exceptions,
    })
}

fn positive(raw: &Table, key: &str, errors: &mut Vec<String>) -> Option<usize> {
    match raw.get(key).and_then(Value::as_integer) {
        Some(value) if value > 0 => Some(value as usize),
        _ => {
            errors.push(format!("function_size.{key} 必须是正整数"));
            None
        }
    }
}

fn parse_rules(value: Option<&Value>, label: &str, errors: &mut Vec<String>) -> Vec<FunctionRule> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Value::Array(values) = value else {
        errors.push(format!("{label} 必须是 table 数组"));
        return Vec::new();
    };
    let mut rules = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if let Some(rule) = parse_rule(value, &format!("{label}[{index}]"), errors) {
            rules.push(rule);
        }
    }
    let duplicates = duplicate_rules(&rules);
    if !duplicates.is_empty() {
        errors.push(format!("{label} 不得重复：{}", format_rules(&duplicates)));
    }
    rules
}

fn parse_rule(value: &Value, label: &str, errors: &mut Vec<String>) -> Option<FunctionRule> {
    let Some(value) = value.as_table() else {
        errors.push(format!("{label} 必须是 TOML table"));
        return None;
    };
    let Some(path) = value.get("path").and_then(Value::as_str) else {
        errors.push(format!("{label}.path 必须是规范 Rust 相对路径"));
        return None;
    };
    if !valid_relative_rust_path(path) {
        errors.push(format!("{label}.path 必须是规范 Rust 相对路径"));
        return None;
    }
    let Some(symbol) = value
        .get("symbol")
        .and_then(Value::as_str)
        .filter(|symbol| !symbol.trim().is_empty())
    else {
        errors.push(format!("{label}.symbol 必须是非空字符串"));
        return None;
    };
    Some(FunctionRule {
        path: path.to_owned(),
        symbol: symbol.to_owned(),
    })
}

fn parse_exceptions(
    value: Option<&Value>,
    max_exception_days: usize,
    errors: &mut Vec<String>,
) -> Vec<FunctionException> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Value::Array(values) = value else {
        errors.push("function_size.exceptions 必须是 table 数组".into());
        return Vec::new();
    };
    let today = Local::now().date_naive();
    let mut result = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let label = format!("function_size.exceptions[{index}]");
        let Some(rule) = parse_rule(value, &label, errors) else {
            continue;
        };
        let entry = value.as_table().expect("parse_rule 已验证 table");
        let Some(current_lines) = entry
            .get("current_lines")
            .and_then(Value::as_integer)
            .filter(|value| *value > 0)
            .map(|value| value as usize)
        else {
            errors.push(format!("{label}.current_lines 必须是正整数"));
            continue;
        };
        if !entry
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| !reason.trim().is_empty())
        {
            errors.push(format!("{label}.reason 必须是非空字符串"));
            continue;
        }
        let Some(expires) = entry.get("expires").and_then(toml_date) else {
            errors.push(format!("{label}.expires 必须是 TOML 日期"));
            continue;
        };
        if expires < today {
            errors.push(format!("{label} 已过期：{}", expires.format("%Y-%m-%d")));
        }
        if expires > today + Duration::days(max_exception_days as i64) {
            errors.push(format!(
                "{label}.expires 不得超过 {max_exception_days} 天：{}",
                expires.format("%Y-%m-%d")
            ));
        }
        result.push(FunctionException {
            rule,
            current_lines,
        });
    }
    result
}

fn toml_date(value: &Value) -> Option<NaiveDate> {
    let datetime = value.as_datetime()?;
    if datetime.time.is_some() {
        return None;
    }
    let date = datetime.date?;
    NaiveDate::from_ymd_opt(date.year.into(), date.month.into(), date.day.into())
}

fn valid_relative_rust_path(value: &str) -> bool {
    !value.contains('\\')
        && value.ends_with(".rs")
        && !Path::new(value).is_absolute()
        && !Path::new(value)
            .components()
            .any(|component| component == std::path::Component::ParentDir)
}

fn duplicate_rules(rules: &[FunctionRule]) -> Vec<FunctionRule> {
    let mut seen = BTreeSet::new();
    rules
        .iter()
        .filter(|rule| !seen.insert((*rule).clone()))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn format_rules(rules: &[FunctionRule]) -> String {
    rules
        .iter()
        .map(|rule| format!("{}:{}", rule.path, rule.symbol))
        .collect::<Vec<_>>()
        .join(", ")
}

fn mutable_function_records(
    records: &[FunctionRecord],
    frozen: &BTreeSet<String>,
    retained: &BTreeSet<(String, String)>,
) -> Vec<FunctionRecord> {
    records
        .iter()
        .filter(|record| {
            !frozen.contains(&record.path)
                || retained.contains(&(record.path.clone(), record.symbol.clone()))
        })
        .cloned()
        .collect()
}

pub(crate) fn validate_functions(
    policy: &FunctionPolicy,
    records: &[FunctionRecord],
    changed: Option<&BTreeMap<String, Vec<(usize, usize)>>>,
    errors: &mut Vec<String>,
) -> (usize, usize) {
    let by_key = records
        .iter()
        .map(|record| ((record.path.clone(), record.symbol.clone()), record))
        .collect::<BTreeMap<_, _>>();
    let orchestration_keys = policy
        .orchestrations
        .iter()
        .map(|rule| (rule.path.clone(), rule.symbol.clone()))
        .collect::<BTreeSet<_>>();
    let exception_by_key = policy
        .exceptions
        .iter()
        .map(|exception| {
            (
                (exception.rule.path.clone(), exception.rule.symbol.clone()),
                exception,
            )
        })
        .collect::<BTreeMap<_, _>>();
    for (path, symbol) in orchestration_keys
        .iter()
        .filter(|key| !by_key.contains_key(*key))
    {
        errors.push(format!("编排函数登记已失效：{path}:{symbol}"));
    }
    for (path, symbol) in exception_by_key
        .keys()
        .filter(|key| !by_key.contains_key(*key))
    {
        errors.push(format!("函数长度例外已失效：{path}:{symbol}"));
    }
    let mut checked = 0;
    let mut violations = 0;
    for record in records {
        let key = (record.path.clone(), record.symbol.clone());
        if let Some(exception) = exception_by_key.get(&key) {
            if record.lines > exception.current_lines {
                errors.push(format!(
                    "函数长度例外只能缩短：{}:{} {}（登记 {}，实际 {}）",
                    record.path,
                    record.start_line,
                    record.symbol,
                    exception.current_lines,
                    record.lines
                ));
                violations += 1;
            }
            continue;
        }
        if policy.mode == "changed"
            && changed.is_some()
            && !function_changed(record, changed.expect("changed 已检查"))
        {
            continue;
        }
        checked += 1;
        let (limit, category) = if orchestration_keys.contains(&key) {
            (policy.orchestration_max_lines, "编排函数")
        } else if record.private {
            (policy.private_max_lines, "private helper")
        } else {
            (policy.public_max_lines, "普通函数")
        };
        if record.lines > limit {
            errors.push(format!(
                "{category}超过 {limit} 行：{}:{} {}（{} 行）",
                record.path, record.start_line, record.symbol, record.lines
            ));
            violations += 1;
        }
    }
    (checked, violations)
}

fn function_changed(
    record: &FunctionRecord,
    changed: &BTreeMap<String, Vec<(usize, usize)>>,
) -> bool {
    changed
        .get(&record.path)
        .into_iter()
        .flatten()
        .any(|(start, end)| *start <= record.end_line && *end >= record.start_line)
}
