use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

use super::{
    git::{self, Reply},
    inventory::{self, Inventory},
    model::{Count, FileRecord, Kind, Percent, Report, Summary, ratio_text},
    policy::{self, Policy},
};

pub(crate) fn build(
    policy: Policy,
    inventory: Inventory,
    files: Vec<FileRecord>,
    minimum: Option<Percent>,
) -> Result<Report> {
    let mut own = Summary::default();
    let mut vendor = Summary::default();
    let mut total = Count::default();
    for file in &files {
        total.add(file.bytes)?;
        add(if file.vendor { &mut vendor } else { &mut own }, file)?;
    }
    Ok(Report {
        format_version: 1,
        policy_sha256: policy::digest(policy::SOURCE.as_bytes()),
        policy,
        head: inventory.head,
        source_sha256: inventory::fingerprint(&files),
        total,
        rust_percent: ratio_text(own.rust.bytes, own.code.bytes),
        min_rust_percent: minimum.map(Percent::text),
        threshold_passed: minimum.map(|minimum| minimum.accepts(own.rust.bytes, own.code.bytes)),
        self_maintained: own,
        vendor,
        ignored_tracked: inventory.ignored.into_iter().collect(),
        deleted_tracked: inventory.deleted.into_iter().collect(),
        files,
    })
}

fn add(summary: &mut Summary, file: &FileRecord) -> Result<()> {
    summary.total.add(file.bytes)?;
    summary
        .categories
        .entry(file.kind.label().into())
        .or_default()
        .add(file.bytes)?;
    let extension = Path::new(&file.path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("无扩展名")
        .to_ascii_lowercase();
    summary
        .extensions
        .entry(extension)
        .or_default()
        .add(file.bytes)?;
    if file.kind == Kind::Code {
        summary.code.add(file.bytes)?;
        let language = file.language.as_deref().ok_or("代码文件缺少语言分类")?;
        let language_bytes = file
            .bytes
            .checked_sub(file.embedded_lua_bytes.unwrap_or(0))
            .ok_or("内联 Lua 字节超过宿主文件长度")?;
        summary
            .languages
            .entry(language.into())
            .or_default()
            .add(language_bytes)?;
        if let Some(bytes) = file.embedded_lua_bytes {
            summary
                .languages
                .entry("Lua".into())
                .or_default()
                .add(bytes)?;
        }
        if language == "Rust" {
            summary.rust.add(language_bytes)?;
        }
        let group = match (file.test, file.generated, file.template) {
            (true, _, _) => "测试",
            (false, _, true) => "模板",
            (false, true, false) => "生成",
            (false, false, false) => "手写",
        };
        summary
            .code_groups
            .entry(group.into())
            .or_default()
            .add(file.bytes)?;
    }
    Ok(())
}

pub(crate) fn render(report: &Report) -> String {
    let mut lines = vec![
        format!(
            "语言/文件构成报告：{}；口径 SHA256={}",
            report.policy.id, report.policy_sha256
        ),
        report.policy.measurement.clone(),
        "完整分类规则：architecture/language-policy.toml；JSON 报告同时包含规则与逐文件字节及摘要。".into(),
        report.policy.inventory.clone(),
        report.policy.denominator.clone(),
        report.policy.embedded_lua_rule.clone(),
        report.policy.legacy_reference.clone(),
        format!(
            "源码 HEAD={}；观测摘要={}",
            report.head.as_deref().unwrap_or("空仓库尚无提交"),
            report.source_sha256
        ),
        format!(
            "全量非 ignored 文件：{} 个，{} 字节",
            report.total.files, report.total.bytes
        ),
        format!(
            "自维护代码：{} 字节；Rust：{} 字节；Rust 占比：{}",
            report.self_maintained.code.bytes,
            report.self_maintained.rust.bytes,
            report
                .rust_percent
                .as_ref()
                .map(|value| format!("{value}%（向下保留六位小数）"))
                .unwrap_or("不适用（代码分母为零）".into())
        ),
    ];
    for (name, summary) in [
        ("自维护", &report.self_maintained),
        ("vendor（不参与阈值）", &report.vendor),
    ] {
        lines.push(format!(
            "{name}：{} 个文件，{} 字节",
            summary.total.files, summary.total.bytes
        ));
        for (category, count) in &summary.categories {
            lines.push(format!(
                "  {category}：{} 个文件，{} 字节",
                count.files, count.bytes
            ));
        }
        for (language, count) in &summary.languages {
            lines.push(format!(
                "  代码语言 {language}：{} 个文件，{} 字节",
                count.files, count.bytes
            ));
        }
        for (group, count) in &summary.code_groups {
            lines.push(format!(
                "  代码分组 {group}：{} 个文件，{} 字节",
                count.files, count.bytes
            ));
        }
    }
    let fallback = report.files.iter().filter(|file| file.fallback).count();
    lines.push(format!("保守回退分类：{fallback} 个文件；已排除 ignored 已跟踪文件：{} 个；已删除已跟踪文件：{} 个", report.ignored_tracked.len(), report.deleted_tracked.len()));
    lines.push(match &report.min_rust_percent {
        Some(minimum) => format!(
            "显式 Rust 阈值 {minimum}%：{}",
            if report.threshold_passed == Some(true) {
                "通过"
            } else {
                "未通过"
            }
        ),
        None => "仅报告，未请求阈值检查；不宣称已达到 80%。".into(),
    });
    lines.join("\n")
}

pub(crate) fn validate_output(
    root: &Path,
    path: &Path,
    git: &mut impl FnMut(&[&str]) -> Result<Reply>,
) -> Result<()> {
    validate_local_test_path(path, root, LocalTestPathKind::OutputFile)
        .map_err(|error| format!("报告输出必须为忽略目录内的新文件：{error}"))?;
    if path.try_exists()? || !path.parent().is_some_and(Path::is_dir) {
        return Err("报告输出必须为现有忽略目录下的新文件；不会覆盖文件或创建父目录".into());
    }
    let relative = path
        .strip_prefix(root)?
        .to_str()
        .ok_or("报告输出路径不是 UTF-8")?
        .replace('\\', "/");
    git::success(git(&["check-ignore", "--no-index", "--", &relative])?)?;
    let tracked = git::success(git(&["ls-files", "--cached", "-z", "--", &relative])?)?;
    if !tracked.is_empty() {
        return Err("报告输出不能是已跟踪文件，即使当前符合 ignore 规则".into());
    }
    Ok(())
}

pub(crate) fn write_new(root: &Path, path: &Path, report: &Report) -> Result<()> {
    let mut body = serde_json::to_vec_pretty(report)?;
    body.push(b'\n');
    validate_local_test_path(path, root, LocalTestPathKind::OutputFile)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(&body)
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            format!(
                "新报告写入未完成，保留文件供核验，不自动覆盖或重放 {}：{error}",
                path.display()
            )
        })?;
    validate_local_test_path(path, root, LocalTestPathKind::ExistingFile)?;
    if fs::read(path)? != body {
        return Err("新报告内容变化，拒绝报告成功并保留现场".into());
    }
    Ok(())
}
