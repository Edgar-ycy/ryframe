use std::{collections::BTreeMap, path::PathBuf};

use serde::Deserialize;

use super::super::model::{
    CiCommand, CliError, RequiredAction, RequiredEvent, RequiredJobResult, RequiredNeed,
    RequiredOptions, ResourceGateReplayOptions,
};

pub(super) fn parse_ci(args: &[String]) -> Result<CiCommand, CliError> {
    match args {
        [command] if command == "plan" => Ok(CiCommand::Plan),
        [command] if command == "preflight" => Ok(CiCommand::Preflight),
        [command] if command == "rust-gate" => Ok(CiCommand::RustGate),
        [command] if command == "resource-gate" => Ok(CiCommand::ResourceGate),
        [command, operation, rest @ ..] if command == "resource-gate" && operation == "replay" => {
            parse_resource_gate_replay(rest).map(CiCommand::ResourceGateReplay)
        }
        [command] if command == "integration" => Ok(CiCommand::Integration),
        [command] if command == "consumer-contract" => Ok(CiCommand::ConsumerContract),
        [command, rest @ ..] if command == "required" => {
            parse_required(rest).map(CiCommand::Required)
        }
        _ => Err(CliError::new(
            "用法：cargo xtask check ci <plan|preflight|rust-gate|resource-gate|integration|consumer-contract|required>；资源门禁回放使用 resource-gate replay --manifest <文件> --work-dir <目录> --report <文件> [--activation-gate]",
        )),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequiredNeedDocument {
    result: String,
    #[serde(default)]
    outputs: BTreeMap<String, String>,
}

fn parse_required(args: &[String]) -> Result<RequiredOptions, CliError> {
    let values = required_named_options(args)?;
    let event = RequiredEvent::parse(required_value(&values, "--event")?)?;
    let action = RequiredAction::parse(event, values.get("--action").map(String::as_str))?;
    let needs = parse_required_needs(required_value(&values, "--needs-json")?)?;
    Ok(RequiredOptions {
        event,
        action,
        needs,
    })
}

fn required_named_options(args: &[String]) -> Result<BTreeMap<String, String>, CliError> {
    let allowed = ["--event", "--action", "--needs-json"];
    let mut values = BTreeMap::new();
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if !allowed.contains(&option) {
            return Err(CliError::new(format!("未知参数：{option}")));
        }
        let value = args
            .get(index + 1)
            .filter(|value| option == "--action" || !value.trim().is_empty())
            .filter(|value| value.is_empty() || !value.starts_with('-'))
            .ok_or_else(|| CliError::new(format!("{option} 缺少取值")))?;
        if values.insert(option.to_owned(), value.clone()).is_some() {
            return Err(CliError::new(format!("{option} 不能重复")));
        }
        index += 2;
    }
    Ok(values)
}

fn required_value<'a>(
    values: &'a BTreeMap<String, String>,
    option: &str,
) -> Result<&'a str, CliError> {
    values
        .get(option)
        .map(String::as_str)
        .ok_or_else(|| CliError::new(format!("缺少必需参数 {option}")))
}

fn parse_required_needs(value: &str) -> Result<BTreeMap<String, RequiredNeed>, CliError> {
    let document = serde_json::from_str::<BTreeMap<String, RequiredNeedDocument>>(value)
        .map_err(|error| CliError::new(format!("--needs-json 无效：{error}")))?;
    let mut needs = BTreeMap::new();
    for (name, need) in document {
        validate_need(&name, &need)?;
        needs.insert(
            name,
            RequiredNeed {
                result: RequiredJobResult::parse(&need.result)?,
                outputs: need.outputs,
            },
        );
    }
    Ok(needs)
}

fn validate_need(name: &str, need: &RequiredNeedDocument) -> Result<(), CliError> {
    if name.trim().is_empty() {
        return Err(CliError::new("needs JSON 的 job 名称必须是非空字符串"));
    }
    if need.outputs.keys().any(|name| name.trim().is_empty())
        || need.outputs.values().any(|value| value.is_empty())
    {
        return Err(CliError::new(
            "needs JSON 的 output 名称和值必须是非空字符串",
        ));
    }
    Ok(())
}

fn parse_resource_gate_replay(args: &[String]) -> Result<ResourceGateReplayOptions, CliError> {
    let mut manifest = None;
    let mut work_dir = None;
    let mut report = None;
    let mut activation_gate = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--manifest" | "--work-dir" | "--report" => {
                let option = args[index].as_str();
                let value = required_argument(args.get(index + 1), option)?;
                let target = match option {
                    "--manifest" => &mut manifest,
                    "--work-dir" => &mut work_dir,
                    "--report" => &mut report,
                    _ => unreachable!(),
                };
                if target.replace(PathBuf::from(value)).is_some() {
                    return Err(CliError::new(format!("{option} 不能重复")));
                }
                index += 2;
            }
            "--activation-gate" if !activation_gate => {
                activation_gate = true;
                index += 1;
            }
            "--activation-gate" => return Err(CliError::new("--activation-gate 不能重复")),
            unknown => return Err(CliError::new(format!("未知参数：{unknown}"))),
        }
    }
    Ok(ResourceGateReplayOptions {
        manifest: manifest.ok_or_else(|| CliError::new("缺少必需参数 --manifest"))?,
        work_dir: work_dir.ok_or_else(|| CliError::new("缺少必需参数 --work-dir"))?,
        report: report.ok_or_else(|| CliError::new("缺少必需参数 --report"))?,
        activation_gate,
    })
}

fn required_argument<'a>(value: Option<&'a String>, option: &str) -> Result<&'a str, CliError> {
    value
        .filter(|value| !value.trim().is_empty() && !value.starts_with('-'))
        .map(String::as_str)
        .ok_or_else(|| CliError::new(format!("{option} 缺少取值")))
}
