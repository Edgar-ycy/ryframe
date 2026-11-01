use super::super::super::model::{CliError, FullStackConfigOptions};
use std::{collections::BTreeMap, path::PathBuf};

pub(super) fn parse(args: &[String]) -> Result<FullStackConfigOptions, CliError> {
    let mut values = BTreeMap::new();
    let mut flags = std::collections::BTreeSet::new();
    let mut index = 0;
    while index < args.len() {
        let key = args[index].as_str();
        if matches!(key, "--write" | "--plan") {
            if !flags.insert(key) {
                return Err(CliError::new("重复执行授权参数"));
            }
            index += 1;
            continue;
        }
        if !matches!(
            key,
            "--template"
                | "--runtime-dir"
                | "--run-id"
                | "--attempt"
                | "--source-sha"
                | "--source-backend"
                | "--source-frontend"
                | "--mysql-container"
                | "--redis-container"
                | "--s3-container"
        ) {
            return Err(CliError::new(
                "全栈配置存在未知参数；请查看 full-stack-config 用法",
            ));
        }
        let value = args
            .get(index + 1)
            .filter(|v| !v.is_empty() && !v.starts_with('-'))
            .ok_or_else(|| CliError::new(format!("{key} 缺少取值")))?;
        if values.insert(key, value.clone()).is_some() {
            return Err(CliError::new(format!("{key} 不能重复")));
        }
        index += 2;
    }
    let mut take = |key| {
        values
            .remove(key)
            .ok_or_else(|| CliError::new(format!("缺少必需参数 {key}")))
    };
    let options = FullStackConfigOptions {
        template: PathBuf::from(take("--template")?),
        runtime_dir: PathBuf::from(take("--runtime-dir")?),
        run_id: take("--run-id")?,
        attempt: take("--attempt")?,
        source_sha: take("--source-sha")?,
        source_backend: PathBuf::from(take("--source-backend")?),
        source_frontend: PathBuf::from(take("--source-frontend")?),
        mysql_container: take("--mysql-container")?,
        redis_container: take("--redis-container")?,
        s3_container: take("--s3-container")?,
        write: flags.contains("--write"),
        plan: flags.contains("--plan"),
    };
    crate::ci::full_stack_config::plan(&options)
        .map_err(|error| CliError::new(format!("全栈配置参数无效：{error}")))?;
    Ok(options)
}
