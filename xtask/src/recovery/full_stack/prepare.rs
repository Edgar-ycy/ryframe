use super::{
    Port,
    context::{Context, journal},
};
use crate::{Result, ci::databases::snapshot::Frozen, recovery::clone::native::json as documents};
use serde_json::{Value, json};
use std::fs;

pub(super) fn run(context: &Context, port: &mut impl Port) -> Result<()> {
    let frozen = context.frozen()?;
    run_frozen(context, port, &frozen)
}

pub(super) fn run_frozen(context: &Context, port: &mut impl Port, frozen: &Frozen) -> Result<()> {
    validate(frozen.settings())?;
    context.container("redis")?;
    if frozen.settings()["object_storage"]["backend"] != "local" {
        context.container("s3")?;
    }
    fs::create_dir_all(&context.output)?;
    let path = context.output.join("ci-prepare.json");
    documents::write_new(
        &path,
        &json!({"format_version":1,"state":"preparing","phase":"build"}),
    )?;
    let result = phases(context, port, frozen);
    if let Err(error) = &result {
        let mut evidence = documents::read(&path)?;
        evidence["state"] = json!("needs-reconciliation");
        evidence["automatic_retry_permitted"] = json!(false);
        journal(&path, &evidence)?;
        return Err(format!(
            "全栈 prepare 未完成：{error}；保留阶段、数据库和对象收据，禁止自动重放"
        )
        .into());
    }
    journal(&path, &json!({"format_version":1,"state":"prepared"}))?;
    result
}

pub(super) fn phases(context: &Context, port: &mut impl Port, frozen: &Frozen) -> Result<()> {
    let journal_path = context.output.join("ci-prepare.json");
    let stage = |name: &str| {
        journal(
            &journal_path,
            &json!({"format_version":1,"state":"write-unknown","phase":name,"automatic_retry_permitted":false}),
        )
    };
    let binaries = port.leaf("build")?;
    stage("storage")?;
    port.storage(frozen)?;
    stage("databases")?;
    port.databases(frozen)?;
    let config = vec![
        "--config-dir".to_owned(),
        context.output.join("config").to_string_lossy().into_owned(),
    ];
    let redis = frozen.settings()["redis"]["database"]
        .as_u64()
        .ok_or("Redis 编号无效")?;
    let sentinel = frozen.settings()["reset"]["redis_outside_sentinel_key"]
        .as_str()
        .ok_or("缺少 Redis 哨兵")?;
    stage("redis-sentinel")?;
    port.command(
        "docker",
        &[
            "exec".into(),
            context.container("redis")?.into(),
            "redis-cli".into(),
            "-n".into(),
            redis.to_string(),
            "SET".into(),
            sentinel.into(),
            "ci-sentinel".into(),
            "NX".into(),
        ],
        None,
    )?;
    let reset = binary(&binaries, "ryframe-reset")?;
    let mut args = config.clone();
    args.push("plan".into());
    let plan = port.command(reset, &args, Some(&context.output.join("reset-plan.log")))?;
    let hash = plan_hash(&plan)?;
    stage("reset")?;
    let mut args = config.clone();
    args.extend([
        "execute".into(),
        "--plan-hash".into(),
        hash,
        "--confirm-reset".into(),
        format!("RESET-RYFRAME-test-{}", frozen.scope()?),
    ]);
    port.command(
        reset,
        &args,
        Some(&context.output.join("reset-execute.log")),
    )?;
    let migrate = binary(&binaries, "ryframe-migrate")?;
    for suffix in [
        vec!["control", "up"],
        vec!["tenant-data", "up", "--all"],
        vec!["control", "verify"],
        vec!["tenant-data", "verify", "--all"],
    ] {
        stage("migrations")?;
        let mut args = config.clone();
        args.extend(suffix.into_iter().map(String::from));
        port.command(migrate, &args, None)?;
    }
    Ok(())
}

pub(super) fn validate(settings: &Value) -> Result<()> {
    if settings["jobs"]["mode"] != "external" {
        return Err("所选配置 jobs.mode 必须为 external".into());
    }
    if settings["redis"]["database"]
        .as_u64()
        .is_none_or(|v| v > 15)
    {
        return Err("Redis 数据库编号必须在0–15之间".into());
    }
    if settings["reset"]["redis_outside_sentinel_key"]
        .as_str()
        .is_none_or(str::is_empty)
    {
        return Err("缺少 reset.redis_outside_sentinel_key".into());
    }
    let storage = &settings["object_storage"];
    if !matches!(
        storage["backend"].as_str(),
        Some("local" | "s3" | "rustfs" | "minio")
    ) {
        return Err("对象存储后端无效".into());
    }
    if storage["backend"] == "local" {
        if !std::path::Path::new(
            storage["local_base_dir"]
                .as_str()
                .ok_or("缺少本地存储路径")?,
        )
        .is_absolute()
        {
            return Err("本地存储必须绑定绝对路径".into());
        }
    } else {
        let endpoint = url::Url::parse(storage["endpoint"].as_str().ok_or("缺少对象端点")?)
            .map_err(|_| "对象端点无效")?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || !matches!(endpoint.host_str(), Some("127.0.0.1" | "localhost"))
            || endpoint.path() != "/"
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
        {
            return Err("CI 对象端点必须指向无凭据和路径前缀的 loopback 实例".into());
        }
    }
    Ok(())
}

pub(super) fn plan_hash(plan: &str) -> Result<String> {
    regex::Regex::new(r"(?m)^plan_hash=([0-9a-f]{64})$")?
        .captures_iter(plan)
        .last()
        .map(|v| v[1].to_owned())
        .ok_or_else(|| "reset plan 缺少有效 hash；未执行 reset".into())
}

fn binary<'a>(binaries: &'a Value, name: &str) -> Result<&'a str> {
    binaries[name]
        .as_str()
        .filter(|v| std::path::Path::new(v).is_absolute())
        .ok_or_else(|| format!("缺少明确的 {name} 构建产物").into())
}
