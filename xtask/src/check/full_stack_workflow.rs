use std::{fs, path::Path};

use super::supply_chain::yaml::{self, YamlResult, YamlValue};
use crate::Result;

const WORKFLOW: &str = ".github/workflows/extended-ci.yml";
const SOURCE_PAIR: &str = r#""$RUNNER_TEMP/ryframe-full-stack/source-pair.json""#;
const FRONTEND_ROOT: &str = r#""$GITHUB_WORKSPACE/frontend""#;
const FIXTURE_OUTPUT: &str = r#""$GITHUB_WORKSPACE/backend/.local-tests/business-fixture""#;
const DIAGNOSTICS: &str = "保存全栈诊断产物";
const TOOL_TARGET: &str = "$RUNNER_TEMP/ryframe-full-stack-tool";
const CORE: &str = "${{ matrix.fixture == 'core' }}";
const BUSINESS: &str = "${{ matrix.fixture == 'business' }}";
const CORE_BUILD: &str = r#"corepack pnpm build --source-commit "$(git rev-parse HEAD)""#;
const DIAGNOSTIC_PATHS: &[&str] = &[
    "${{ runner.temp }}/ryframe-full-stack",
    "!${{ runner.temp }}/ryframe-full-stack/config/**",
    "!${{ runner.temp }}/ryframe-full-stack/worker-config/**",
    "!**/.local-tests/frontend-tools.json",
    "!**/.local-tests/full-stack-config/**",
    "backend/.local-tests/business-fixture/fixture.json",
    "backend/.local-tests/business-fixture/prepare.log",
];

pub(super) fn run(root: &Path) -> Result<()> {
    let path = root.join(WORKFLOW);
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    validate(&source).map_err(|error| format!("{}：{error}", path.display()).into())
}

fn validate(source: &str) -> YamlResult<()> {
    let workflow = yaml::parse(source)?;
    let job = workflow
        .get("jobs")
        .and_then(|jobs| jobs.get("full-stack-e2e"))
        .ok_or("缺少 full-stack-e2e 工作流任务")?;
    let steps = job
        .get("steps")
        .and_then(YamlValue::as_sequence)
        .ok_or("full-stack-e2e.steps 必须是步骤序列")?;
    let upload = validate_upload(steps)?;
    validate_stages(steps, upload)?;
    validate_matrix(job)
}

fn validate_stages(steps: &[YamlValue], upload: usize) -> YamlResult<()> {
    let record = pair_step(
        steps,
        "cargo xtask check release ci record-pair",
        "--output",
    )?;
    let verify = pair_step(steps, "cargo xtask check release ci verify-pair", "--input")?;
    let (fixture, fixture_step) = command(steps, "cargo xtask check recovery fixture")?;
    required_text(fixture_step, "working-directory", "backend", "业务夹具")?;
    required_text(fixture_step, "if", BUSINESS, "业务夹具")?;
    required_argument(
        fixture_step,
        "--frontend-dir",
        Some(FRONTEND_ROOT),
        "业务夹具",
    )?;
    required_argument(
        fixture_step,
        "--output-dir",
        Some(FIXTURE_OUTPUT),
        "业务夹具",
    )?;
    required_argument(fixture_step, "--write", None, "业务夹具")?;

    let (configure, _) = matrix_backend_step(steps, "cargo xtask check ci full-stack-config")?;
    let tool_build = matrix_tool_step(
        steps,
        &format!(r#"cargo build --locked -p xtask --bin xtask --target-dir "{TOOL_TARGET}""#),
        None,
    )?;
    let tool = format!(r#""{TOOL_TARGET}/debug/xtask" check recovery full-stack"#);
    let prepare = matrix_tool_step(steps, &format!("{tool} prepare"), None)?;
    let start = matrix_tool_step(steps, &format!("{tool} start"), None)?;
    let collect = matrix_tool_step(steps, &format!("{tool} collect"), Some("${{ always() }}"))?;
    let (core_build, step) = command(steps, "corepack pnpm build")?;
    required_text(
        step,
        "working-directory",
        "${{ matrix.frontend }}",
        "前端生产构建",
    )?;
    required_text(step, "if", CORE, "前端生产构建")?;
    required_text(step, "run", CORE_BUILD, "前端生产构建")?;
    let business_build = matrix_tool_step(steps, &format!("{tool} frontend"), Some(BUSINESS))?;

    if ![
        record,
        fixture,
        configure,
        tool_build,
        prepare,
        start,
        core_build,
        business_build,
        verify,
        collect,
        upload,
    ]
    .windows(2)
    .all(|pair| pair[0] < pair[1])
    {
        return Err(
            "全栈步骤顺序必须为来源登记、业务夹具、配置、工具构建、准备、启动、前端构建、来源复核、收集、诊断上传"
                .into(),
        );
    }
    Ok(())
}

fn matrix_backend_step<'a>(
    steps: &'a [YamlValue],
    name: &str,
) -> YamlResult<(usize, &'a YamlValue)> {
    let (index, step) = command(steps, name)?;
    required_text(step, "working-directory", "${{ matrix.backend }}", name)?;
    Ok((index, step))
}

fn matrix_tool_step(steps: &[YamlValue], run: &str, condition: Option<&str>) -> YamlResult<usize> {
    let (index, step) = matrix_backend_step(steps, run)?;
    required_text(step, "run", run, run)?;
    if let Some(condition) = condition {
        required_text(step, "if", condition, run)?;
    } else if step.get("if").is_some() {
        return Err(format!("全栈 {run} 必须无条件执行"));
    }
    Ok(index)
}

fn pair_step(steps: &[YamlValue], name: &str, receipt_flag: &str) -> YamlResult<usize> {
    let (index, step) = command(steps, name)?;
    required_text(step, "working-directory", "backend", name)?;
    required_argument(step, receipt_flag, Some(SOURCE_PAIR), name)?;
    required_argument(step, "--frontend-dir", Some(FRONTEND_ROOT), name)?;
    Ok(index)
}

fn validate_upload(steps: &[YamlValue]) -> YamlResult<usize> {
    let (index, upload) = one_step(
        steps,
        |step| text(step, "name") == Some(DIAGNOSTICS),
        DIAGNOSTICS,
    )?;
    if !text(upload, "uses").is_some_and(|uses| uses.starts_with("actions/upload-artifact@")) {
        return Err(format!("{DIAGNOSTICS}必须使用 actions/upload-artifact"));
    }
    required_text(upload, "if", "${{ always() }}", DIAGNOSTICS)?;
    let settings = upload.get("with").ok_or("缺少全栈诊断产物上传参数")?;
    required_text(
        settings,
        "name",
        "ryframe-full-stack-${{ github.run_id }}-${{ github.run_attempt }}${{ matrix.artifact_suffix }}",
        DIAGNOSTICS,
    )?;
    if settings.get("include-hidden-files") != Some(&YamlValue::Bool(true)) {
        return Err(format!("{DIAGNOSTICS}必须包含隐藏文件"));
    }
    let paths = text(settings, "path").ok_or("缺少全栈诊断产物路径")?;
    if !paths
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .eq(DIAGNOSTIC_PATHS.iter().copied())
    {
        return Err(
            "全栈诊断产物必须使用固定运行目录、私有配置排除项和业务夹具公开收据，且不得上传派生仓库或已退役的 playwright-real 路径".into(),
        );
    }
    Ok(index)
}

fn validate_matrix(job: &YamlValue) -> YamlResult<()> {
    let rows = job
        .get("strategy")
        .and_then(|strategy| strategy.get("matrix"))
        .and_then(|matrix| matrix.get("include"))
        .and_then(YamlValue::as_sequence)
        .ok_or("缺少全栈 core/business 矩阵")?;
    for (name, suffix) in [("core", ""), ("business", "-business")] {
        let (_, entry) = one_step(rows, |row| text(row, "fixture") == Some(name), name)?;
        required_text(entry, "artifact_suffix", suffix, name)?;
    }
    if rows.len() != 2 {
        return Err("全栈诊断产物只对应 core/business 矩阵".into());
    }
    Ok(())
}

fn command<'a>(steps: &'a [YamlValue], name: &str) -> YamlResult<(usize, &'a YamlValue)> {
    one_step(
        steps,
        |step| {
            command_line(step).is_some_and(|run| {
                run.split_ascii_whitespace()
                    .take(name.split_ascii_whitespace().count())
                    .eq(name.split_ascii_whitespace())
            })
        },
        name,
    )
}

fn one_step<'a>(
    steps: &'a [YamlValue],
    matches: impl Fn(&YamlValue) -> bool,
    name: &str,
) -> YamlResult<(usize, &'a YamlValue)> {
    let mut selected = steps.iter().enumerate().filter(|(_, step)| matches(step));
    let only = selected
        .next()
        .ok_or_else(|| format!("缺少全栈步骤或矩阵项：{name}"))?;
    if selected.next().is_some() {
        return Err(format!("全栈步骤或矩阵项重复：{name}"));
    }
    Ok(only)
}

fn required_text(value: &YamlValue, key: &str, expected: &str, label: &str) -> YamlResult<()> {
    if text(value, key) != Some(expected) {
        return Err(format!("全栈 {label} 的 {key} 必须为 {expected}"));
    }
    Ok(())
}

fn text<'a>(value: &'a YamlValue, key: &str) -> Option<&'a str> {
    value.get(key).and_then(YamlValue::as_str)
}

fn command_line(step: &YamlValue) -> Option<&str> {
    text(step, "run")?.trim_start().lines().next()
}

// 只提取现有直接命令的固定选项。保留路径引号，确保 Shell 展开变量并保留空格。
fn required_argument(
    step: &YamlValue,
    flag: &str,
    expected: Option<&str>,
    label: &str,
) -> YamlResult<()> {
    let run = command_line(step).ok_or_else(|| format!("缺少全栈 {label} 的 run"))?;
    let mut words = run
        .split_ascii_whitespace()
        .take_while(|word| !word.starts_with('#'));
    let option = |word: &str| {
        word == flag
            || word
                .strip_prefix(flag)
                .is_some_and(|tail| tail.starts_with('='))
    };
    let valid = words.find(|word| option(word)) == Some(flag)
        && expected.is_none_or(|expected| words.next() == Some(expected));
    if !valid || words.any(option) {
        return Err(format!("全栈 {label} 必须恰好一次使用已登记的 {flag} 绑定"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/ci/full_stack_workflow.rs"]
mod tests;
