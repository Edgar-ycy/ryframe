//! 读取当前锁文件、清单及部署引用，核验预发布例外；不启动工具或写入文件。

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use chrono::{Datelike, Local};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::{Date, Month, Weekday, macros::date};

use crate::{Result, process::PreservedFailure};

#[path = "prerelease/deployment.rs"]
pub(crate) mod deployment;
#[path = "prerelease/version.rs"]
pub(crate) mod version;

const ALLOWLIST: &str = "scripts/prerelease_dependency_allowlist.json";
const LAST_DAY_BEFORE_COMMON_ERA: i32 = date!(0001 - 01 - 01).to_julian_day() - 1;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Finding {
    pub(crate) ecosystem: String,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) source: String,
}

impl Finding {
    pub(crate) fn new(ecosystem: &str, name: &str, version: &str, source: &str) -> Self {
        Self {
            ecosystem: ecosystem.into(),
            name: name.into(),
            version: version.into(),
            source: source.into(),
        }
    }
}

/// 复审日期以儒略日保存，使跨出公历 9999 年的 ISO 周仍可参与排序。
pub(crate) type Allowlist = BTreeMap<Finding, i32>;

fn invalid(source: &str, reason: &str) -> Box<dyn std::error::Error> {
    format!("预发布依赖输入无效：{source}：{reason}").into()
}

pub(crate) fn parse_allowlist(source: &str) -> Result<Allowlist> {
    let document: Value =
        serde_json::from_str(source).map_err(|_| invalid(ALLOWLIST, "JSON 格式错误"))?;
    if document.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err(invalid(ALLOWLIST, "不支持的 schema"));
    }
    let entries = document
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(ALLOWLIST, "entries 必须是数组"))?;
    let mut allowed = BTreeMap::new();
    for entry in entries {
        let fields = [
            "ecosystem",
            "name",
            "version",
            "source",
            "review_after",
            "reason",
            "evidence",
        ];
        let values = fields
            .iter()
            .map(|field| {
                entry
                    .get(*field)
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| invalid(ALLOWLIST, "例外缺少非空字符串字段"))
            })
            .collect::<Result<Vec<_>>>()?;
        if !version::is_reference(values[2]) {
            return Err(invalid(ALLOWLIST, "例外版本不是预发布引用"));
        }
        let date = parse_date(values[4])
            .ok_or_else(|| invalid(ALLOWLIST, "review_after 必须是 ISO 日期"))?;
        if !values[6].starts_with("https://") {
            return Err(invalid(ALLOWLIST, "evidence 必须是 HTTPS 来源"));
        }
        let finding = Finding::new(values[0], values[1], values[2], values[3]);
        if allowed.insert(finding, date).is_some() {
            return Err(invalid(ALLOWLIST, "例外重复"));
        }
    }
    Ok(allowed)
}

fn parse_date(value: &str) -> Option<i32> {
    let day = match value.len() {
        8 if value.bytes().all(|b| b.is_ascii_digit()) => calendar_day(value, ""),
        10 if value.as_bytes().get(4) == Some(&b'-') && value.as_bytes().get(7) == Some(&b'-') => {
            calendar_day(value, "-")
        }
        10 if value.as_bytes().get(5) == Some(&b'W') => iso_week_day(value, "-W", "-"),
        8 if value.as_bytes().get(4) == Some(&b'W') => iso_week_day(value, "W", ""),
        // 隐式星期一按原格式追加；这两个历史分支也保留公元前及零年的结果。
        8 if value.as_bytes().get(5) == Some(&b'W') => {
            return iso_week_day(&format!("{value}-1"), "-W", "-");
        }
        7 if value.as_bytes().get(4) == Some(&b'W') => {
            return iso_week_day(&format!("{value}1"), "W", "");
        }
        _ => return None,
    }?;
    (day > LAST_DAY_BEFORE_COMMON_ERA).then_some(day)
}

fn calendar_day(mut input: &str, separator: &str) -> Option<i32> {
    let year = parse_year(&mut input)?;
    input = input.strip_prefix(separator)?;
    let month = Month::try_from(date_number(&mut input, 2)?).ok()?;
    input = input.strip_prefix(separator)?;
    let day = date_number(&mut input, 2)?;
    if !input.is_empty() {
        return None;
    }
    Date::from_calendar_date(year, month, day)
        .ok()
        .map(Date::to_julian_day)
}

fn iso_week_day(mut input: &str, week_prefix: &str, weekday_prefix: &str) -> Option<i32> {
    let year = parse_year(&mut input)?;
    input = input.strip_prefix(week_prefix)?;
    let week = date_number(&mut input, 2)?;
    input = input.strip_prefix(weekday_prefix)?;
    let weekday = date_number(&mut input, 1)?;
    if !input.is_empty() || !(1..=7).contains(&weekday) {
        return None;
    }
    // 先验证 ISO 周的星期一，日序可表达第 9999 年末周跨到下一年的结果。
    let monday = Date::from_iso_week_date(year, week, Weekday::Monday).ok()?;
    Some(monday.to_julian_day() + i32::from(weekday - 1))
}

fn parse_year(input: &mut &str) -> Option<i32> {
    *input = (*input).trim_start();
    let value = *input;
    let sign = match value.as_bytes().first() {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return digits(input, 4)?.parse().ok(),
    };
    *input = &value[1..];
    // 年份的符号之后不再跳过空白，显式符号也解除原四位读取宽度。
    let year: i32 = digits(input, usize::MAX)?.parse().ok()?;
    year.checked_mul(sign)
}

fn date_number(input: &mut &str, width: usize) -> Option<u8> {
    *input = (*input).trim_start();
    digits(input, width)?.parse().ok()
}

fn digits<'a>(input: &mut &'a str, width: usize) -> Option<&'a str> {
    let value = *input;
    let count = value
        .bytes()
        .take(width)
        .take_while(u8::is_ascii_digit)
        .count();
    let (number, remaining) = value.split_at(count);
    *input = remaining;
    (!number.is_empty()).then_some(number)
}

fn read(root: &Path, relative: &str) -> Result<String> {
    fs::read_to_string(root.join(relative)).map_err(|_| invalid(relative, "无法读取 UTF-8 文件"))
}

pub(crate) fn lock_findings(source: &str) -> Result<BTreeSet<Finding>> {
    let document: toml::Value =
        toml::from_str(source).map_err(|_| invalid("Cargo.lock", "TOML 格式错误"))?;
    let mut findings = BTreeSet::new();
    if let Some(packages) = document.get("package") {
        let packages = packages
            .as_array()
            .ok_or_else(|| invalid("Cargo.lock", "package 必须是数组"))?;
        for package in packages {
            let package = package
                .as_table()
                .ok_or_else(|| invalid("Cargo.lock", "package 必须是对象"))?;
            let version = match package.get("version") {
                None => "",
                Some(value) => value
                    .as_str()
                    .ok_or_else(|| invalid("Cargo.lock", "version 必须是字符串"))?,
            };
            if version::is_version(version) {
                let name = package
                    .get("name")
                    .and_then(toml::Value::as_str)
                    .ok_or_else(|| invalid("Cargo.lock", "预发布 package 缺少 name"))?;
                findings.insert(Finding::new("cargo", name, version, "Cargo.lock"));
            }
        }
    }
    Ok(findings)
}

fn entries(root: &Path, relative: &Path) -> Result<Vec<fs::DirEntry>> {
    match fs::read_dir(root.join(relative)) {
        Ok(entries) => entries
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|_| invalid(&relative.to_string_lossy(), "无法遍历目录")),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err(invalid(&relative.to_string_lossy(), "无法读取目录")),
    }
}

fn deployment_files(root: &Path, directory: &Path, files: &mut BTreeSet<PathBuf>) -> Result<()> {
    for entry in entries(root, directory)? {
        let relative = directory.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|_| invalid("deploy", "无法核验文件类型"))?;
        if kind.is_dir() {
            deployment_files(root, &relative, files)?;
        }
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("Dockerfile")
            && entry.path().is_file()
        {
            files.insert(relative);
        }
    }
    Ok(())
}

pub(crate) fn collect(root: &Path) -> Result<BTreeSet<Finding>> {
    let mut findings = lock_findings(&read(root, "Cargo.lock")?)?;
    let mut manifests = BTreeSet::from([PathBuf::from("Cargo.toml")]);
    for entry in entries(root, Path::new("crates"))? {
        if entry.path().is_dir() && entry.path().join("Cargo.toml").exists() {
            manifests.insert(
                Path::new("crates")
                    .join(entry.file_name())
                    .join("Cargo.toml"),
            );
        }
    }
    for path in manifests {
        let relative = path.to_string_lossy().replace('\\', "/");
        for version in version::literals(&read(root, &relative)?) {
            findings.insert(Finding::new(
                "cargo-manifest",
                &relative,
                &version,
                &relative,
            ));
        }
    }
    let mut paths = BTreeSet::new();
    deployment_files(root, Path::new("deploy"), &mut paths)?;
    for entry in entries(root, Path::new(".github/workflows"))? {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "yml" || extension == "yaml")
        {
            paths.insert(Path::new(".github/workflows").join(entry.file_name()));
        }
    }
    for path in paths {
        let relative = path.to_string_lossy().replace('\\', "/");
        findings.extend(deployment::findings(&read(root, &relative)?, &relative));
    }
    Ok(findings)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Evaluation {
    pub(crate) unexpected: Vec<Finding>,
    pub(crate) stale: Vec<Finding>,
    pub(crate) expired: Vec<Finding>,
}

pub(crate) fn evaluate(
    findings: &BTreeSet<Finding>,
    allowed: &Allowlist,
    today: i32,
) -> Evaluation {
    Evaluation {
        unexpected: findings
            .iter()
            .filter(|finding| !allowed.contains_key(*finding))
            .cloned()
            .collect(),
        stale: allowed
            .keys()
            .filter(|finding| !findings.contains(*finding))
            .cloned()
            .collect(),
        expired: allowed
            .iter()
            .filter(|(finding, date)| findings.contains(*finding) && today >= **date)
            .map(|(finding, _)| finding.clone())
            .collect(),
    }
}

pub(crate) fn check(root: &Path, today: i32) -> Result<Vec<String>> {
    let allowed = parse_allowlist(&read(root, ALLOWLIST)?)?;
    let findings = collect(root)?;
    let evaluation = evaluate(&findings, &allowed, today);
    let mut errors = Vec::new();
    for (label, findings) in [
        ("未经评审", evaluation.unexpected),
        ("例外已失效", evaluation.stale),
        ("例外复审已到期", evaluation.expired),
    ] {
        for (index, finding) in findings.into_iter().enumerate() {
            let source = if label == "未经评审" {
                finding.source.as_str()
            } else {
                ALLOWLIST
            };
            // 不输出镜像、action、变量或异常解析内容，避免泄露内嵌凭据。
            errors.push(format!(
                "预发布依赖检查失败：{label}（条目 {}，来源 {}）",
                index + 1,
                source.escape_default()
            ));
        }
    }
    Ok(errors)
}

pub(crate) fn run(root: &Path) -> Result<()> {
    let today = Local::now().date_naive().num_days_from_ce() + LAST_DAY_BEFORE_COMMON_ERA;
    let errors =
        check(root, today).map_err(|error| PreservedFailure::new(error.to_string(), Some(1)))?;
    if !errors.is_empty() {
        return Err(PreservedFailure::new(errors.join("\n"), Some(1)).into());
    }
    println!("预发布依赖检查通过");
    Ok(())
}
