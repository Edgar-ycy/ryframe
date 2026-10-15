use std::{error::Error, path::PathBuf};

pub(super) enum Action {
    Preview {
        module: String,
        output: Option<PathBuf>,
    },
    Apply(PathBuf),
    Inspect(PathBuf),
}

pub(crate) struct Options {
    pub(super) target: Option<String>,
    pub(super) action: Action,
    pub(super) plan: bool,
}

pub(crate) fn parse(args: &[String]) -> Result<Options, Box<dyn Error>> {
    let [scope, _, values @ ..] = args else {
        return Err(super::super::USAGE.into());
    };
    let mut module = None;
    let mut output = None;
    let mut apply = None;
    let mut inspect = None;
    let mut target = None;
    let mut write = false;
    let mut plan = false;
    let mut values = values.iter();
    while let Some(value) = values.next() {
        match value.as_str() {
            "--module" if module.is_none() => module = Some(next_value(&mut values, value)?),
            "--output" if output.is_none() => {
                output = Some(PathBuf::from(next_value(&mut values, value)?))
            }
            "--apply" if apply.is_none() => {
                apply = Some(PathBuf::from(next_value(&mut values, value)?))
            }
            "--inspect" if inspect.is_none() => {
                inspect = Some(PathBuf::from(next_value(&mut values, value)?))
            }
            "--target" if target.is_none() => target = Some(next_value(&mut values, value)?),
            "--write" if !write => write = true,
            "--plan" if !plan => plan = true,
            _ => {
                return Err(format!("未知或重复接管参数：{value}；{}", super::super::USAGE).into());
            }
        }
    }
    match scope.as_str() {
        "control" if target.is_none() => {}
        "tenant-data" if target.is_some() => {}
        _ => {
            return Err(
                "control 接管不接受 --target；tenant-data 接管必须指定一个 --target，不接受 --all"
                    .into(),
            );
        }
    }
    if plan && write {
        return Err("--plan 与 --write 不能同时使用".into());
    }
    let action = match (module, output, apply, inspect) {
        (Some(module), output, None, None) if write == output.is_some() || plan => Action::Preview { module, output },
        (None, None, Some(path), None) if write || plan => Action::Apply(path),
        (None, None, None, Some(path)) if !write => Action::Inspect(path),
        _ => return Err("选择 --module 只读预览；保存计划用 --output <新文件> --write；登记用 --apply <原计划> --write；核验用 --inspect <原计划>，不得混用".into()),
    };
    Ok(Options {
        target,
        action,
        plan,
    })
}

fn next_value(
    values: &mut std::slice::Iter<'_, String>,
    flag: &str,
) -> Result<String, Box<dyn Error>> {
    values
        .next()
        .filter(|value| !value.trim().is_empty() && !value.starts_with("--"))
        .cloned()
        .ok_or_else(|| format!("{flag} 缺少明确参数，尚未读取配置或写入").into())
}
