use std::path::{Component, Path, PathBuf};

use crate::{Result, workspace::root_dir};

#[path = "devex/run.rs"]
mod execution;
#[path = "devex/incremental.rs"]
mod incremental;
#[path = "devex/metadata.rs"]
mod metadata;
#[path = "devex/model.rs"]
mod model;
#[path = "devex/paired.rs"]
mod paired;
#[path = "devex/preflight.rs"]
mod preflight;
#[path = "devex/report.rs"]
mod report;
#[path = "devex/support.rs"]
mod support;

#[allow(unused_imports)]
pub(crate) use model::{
    BaselineContract, CacheState, DevexCommand, DevexPairedOptions, DevexRunOptions, DevexSuite,
    PairedArm,
};
#[allow(unused_imports)]
pub(crate) use preflight::require_frontend_dependencies;

pub(crate) fn parse_command(args: &[String]) -> std::result::Result<DevexCommand, String> {
    let Some(operation) = args.first() else {
        return Err(usage().to_owned());
    };
    match operation.as_str() {
        "run" => parse_run(&args[1..]),
        "paired" => parse_paired(&args[1..]),
        "summarize" => match &args[1..] {
            [run] => Ok(DevexCommand::Summarize { run: run.clone() }),
            _ => Err(usage().to_owned()),
        },
        "compare" => parse_compare(&args[1..]),
        _ => Err(usage().to_owned()),
    }
}

fn parse_paired(args: &[String]) -> std::result::Result<DevexCommand, String> {
    let mut baseline_backend = None;
    let mut candidate_backend = None;
    let mut baseline_frontend = None;
    let mut candidate_frontend = None;
    let mut baseline_contract = None;
    let mut run_args = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--baseline-contract" {
            if baseline_contract.is_some() {
                return Err("--baseline-contract 不能重复".to_owned());
            }
            let value = args
                .get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .ok_or("--baseline-contract 缺少取值")?;
            baseline_contract = Some(
                BaselineContract::parse(value)
                    .ok_or("--baseline-contract 只允许 legacy-cargo-dev-v1")?,
            );
            index += 2;
            continue;
        }
        let slot = match args[index].as_str() {
            "--base-backend" if baseline_backend.is_none() => Some(&mut baseline_backend),
            "--candidate-backend" if candidate_backend.is_none() => Some(&mut candidate_backend),
            "--base-frontend" if baseline_frontend.is_none() => Some(&mut baseline_frontend),
            "--candidate-frontend" if candidate_frontend.is_none() => Some(&mut candidate_frontend),
            _ => None,
        };
        if let Some(slot) = slot {
            let value = args
                .get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| format!("{} 缺少目录", args[index]))?;
            *slot = Some(PathBuf::from(value));
            index += 2;
        } else {
            run_args.push(args[index].clone());
            if let Some(value) = args.get(index + 1) {
                run_args.push(value.clone());
            }
            index += 2;
        }
    }
    let DevexCommand::Run(run) = parse_run(&run_args)? else {
        unreachable!("parse_run 只会返回 Run")
    };
    if let Some(contract) = baseline_contract {
        contract.validate(&run)?;
    }
    let has_frontend_pair = baseline_frontend.is_some() && candidate_frontend.is_some();
    if baseline_frontend.is_some() != candidate_frontend.is_some() {
        return Err("paired 的 --base-frontend 与 --candidate-frontend 必须成对提供".to_owned());
    }
    if run
        .suite
        .definition(&run.variant)
        .map_err(|error| format!("DevEx 变体无效：{error}"))?
        .requires_frontend
        && !has_frontend_pair
    {
        return Err("该 paired suite 必须显式提供两个前端 worktree".to_owned());
    }
    Ok(DevexCommand::Paired(DevexPairedOptions {
        run,
        baseline_backend: baseline_backend.ok_or("paired 缺少 --base-backend")?,
        candidate_backend: candidate_backend.ok_or("paired 缺少 --candidate-backend")?,
        baseline_frontend,
        candidate_frontend,
        baseline_contract,
    }))
}

fn parse_run(args: &[String]) -> std::result::Result<DevexCommand, String> {
    let mut suite = None;
    let mut variant = None;
    let mut cache_state = None;
    let mut runs = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--suite" if suite.is_none() => {
                let value = args.get(index + 1).ok_or("--suite 缺少取值")?;
                suite = Some(
                    DevexSuite::parse(value)
                        .ok_or_else(|| format!("suite 必须是：{}", suite_names()))?,
                );
                index += 2;
            }
            "--variant" if variant.is_none() => {
                let value = args.get(index + 1).ok_or("--variant 缺少取值")?;
                validate_variant(value)?;
                variant = Some(value.clone());
                index += 2;
            }
            "--cache" if cache_state.is_none() => {
                let value = args.get(index + 1).ok_or("--cache 缺少 cold|warm")?;
                cache_state = Some(CacheState::parse(value).ok_or("--cache 只允许 cold 或 warm")?);
                index += 2;
            }
            "--runs" if runs.is_none() => {
                let value = args.get(index + 1).ok_or("--runs 缺少取值")?;
                runs = Some(
                    value
                        .parse::<usize>()
                        .ok()
                        .filter(|value| (1..=50).contains(value))
                        .ok_or("--runs 必须是 1 到 50")?,
                );
                index += 2;
            }
            value => return Err(format!("未知或重复参数：{value}")),
        }
    }
    let suite = suite.ok_or("devex run 缺少 --suite")?;
    let variant = variant.ok_or("devex run 缺少 --variant")?;
    let runs = runs.ok_or("devex run 缺少 --runs")?;
    suite
        .definition(&variant)
        .map_err(|error| format!("{error}；可用变体：{}", suite.variant_help()))?;
    let minimum_runs = suite.minimum_runs(&variant);
    if runs < minimum_runs {
        return Err(format!(
            "suite `{}` 至少需要 {} 次测量，当前为 {runs}",
            suite.as_str(),
            minimum_runs
        ));
    }
    Ok(DevexCommand::Run(DevexRunOptions {
        suite,
        variant,
        cache_state: cache_state.ok_or("devex run 缺少 --cache cold|warm")?,
        runs,
    }))
}

fn parse_compare(args: &[String]) -> std::result::Result<DevexCommand, String> {
    let mut baseline = None;
    let mut candidate = None;
    let mut index = 0;
    while index < args.len() {
        let slot = match args[index].as_str() {
            "--base" if baseline.is_none() => &mut baseline,
            "--candidate" if candidate.is_none() => &mut candidate,
            value => return Err(format!("未知或重复参数：{value}")),
        };
        *slot = Some(
            args.get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| format!("{} 缺少目录", args[index]))?
                .clone(),
        );
        index += 2;
    }
    Ok(DevexCommand::Compare {
        baseline: baseline.ok_or("devex compare 缺少 --base")?,
        candidate: candidate.ok_or("devex compare 缺少 --candidate")?,
    })
}

fn validate_variant(value: &str) -> std::result::Result<(), String> {
    let mut characters = value.chars();
    let valid_start = characters
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric());
    let valid_rest = characters
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    if valid_start && valid_rest && value.len() <= 64 {
        Ok(())
    } else {
        Err("--variant 必须是 1 到 64 位字母、数字、连字符或下划线".to_owned())
    }
}

pub(crate) fn run(command: &DevexCommand, frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    let devex_root = root.join(".local-tests/devex");
    match command {
        DevexCommand::Run(options) => {
            execution::execute(&root, frontend_dir, options)?;
        }
        DevexCommand::Paired(options) => {
            let (baseline, candidate) = paired::execute(&root, frontend_dir, options)?;
            println!("{}", report::compare(&baseline, &candidate)?);
        }
        DevexCommand::Summarize { run } => {
            let run_dir = resolve_run_reference(&devex_root, run)?;
            let summary = report::summarize(&run_dir)?;
            println!(
                "DevEx 摘要：{} / {} / {} 个样本",
                summary.suite.as_str(),
                summary.cache_state.as_str(),
                summary.samples
            );
        }
        DevexCommand::Compare {
            baseline,
            candidate,
        } => {
            let baseline = resolve_run_reference(&devex_root, baseline)?;
            let candidate = resolve_run_reference(&devex_root, candidate)?;
            println!("{}", report::compare(&baseline, &candidate)?);
        }
    }
    Ok(())
}

fn resolve_run_reference(devex_root: &Path, reference: &str) -> Result<PathBuf> {
    if reference.trim().is_empty() {
        return Err("DevEx run 引用不得为空".into());
    }
    let reference = reference.replace('\\', "/");
    let supplied = PathBuf::from(&reference);
    let candidate = if supplied.is_absolute() {
        supplied
    } else {
        let relative = reference
            .strip_prefix(".local-tests/devex/")
            .unwrap_or(&reference);
        let relative = Path::new(relative);
        if relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err("DevEx run 引用必须是 .local-tests/devex 内的规范路径".into());
        }
        devex_root.join(relative)
    };
    let root = devex_root.canonicalize().map_err(|error| {
        format!(
            "DevEx 目录 {} 不存在或不可读：{error}",
            devex_root.display()
        )
    })?;
    let candidate = candidate
        .canonicalize()
        .map_err(|error| format!("DevEx run {} 不存在或不可读：{error}", candidate.display()))?;
    if !candidate.starts_with(&root) || candidate == root {
        return Err("DevEx run 引用越过 .local-tests/devex 边界".into());
    }
    Ok(candidate)
}

pub(crate) fn usage() -> &'static str {
    "cargo xtask devex run --suite <suite> --variant <name> --runs <1..50> --cache <cold|warm>\n\
     cargo xtask devex paired --base-backend <dir> --candidate-backend <dir> [--base-frontend <dir> --candidate-frontend <dir>] [--baseline-contract legacy-cargo-dev-v1] --suite <suite> --variant <name> --runs <1..50> --cache <cold|warm>\n\
     cargo xtask devex summarize <日期/run-id>\n\
     cargo xtask devex compare --base <日期/run-id> --candidate <日期/run-id>"
}

fn suite_names() -> String {
    DevexSuite::ALL
        .into_iter()
        .map(DevexSuite::as_str)
        .collect::<Vec<_>>()
        .join("、")
}

#[allow(unused_imports)]
pub(crate) use incremental::with_source_edit;
#[allow(unused_imports)]
pub(crate) use metadata::{PathNormalizer, filter_environment};
#[allow(unused_imports)]
pub(crate) use paired::abba_pair_order;
#[allow(unused_imports)]
pub(crate) use report::{compare, distribution, summarize};
#[allow(unused_imports)]
pub(crate) use support::{cleanup_successful_sample_target, sample_target};
