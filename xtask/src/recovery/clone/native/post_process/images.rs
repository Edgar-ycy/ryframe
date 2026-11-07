//! 启动准备只读采集原来源和完整复制后像；不发出 reset、迁移或业务写入。
use super::context::Context;
use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::{
        digest,
        export::{
            self,
            transport::{SystemTransport, Transport},
        },
        input::BUCKETS,
        json as document,
        resources::common,
        s3::{Configuration, Operation, Request, app_configuration},
        target_verify::{self, database::SourceBinding, storage::StorageInputs},
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(super) fn verify(context: &Context<'_>, phase: &str) -> Result<()> {
    context.check()?;
    if export::proof::generation_in(&context.source, context.scope)? != context.source_generation {
        return Err("停止的原来源代次不同于复制来源".into());
    }
    let runtime = common::read_bound(
        context.backend,
        &context.source.request["runtime"],
        "原来源运行配置",
    )?;
    let source_configuration = app_configuration::configuration_value(
        &context.source.execution_backend,
        Path::new(document::text(
            &runtime["api_config_dir"],
            "原来源配置目录",
        )?),
        "test",
    )?;
    let mut source_request = context.source.request.clone();
    source_request["target"] = source_request["source"].clone();
    source_request["execution_backend"] = json!({"path":context.source.execution_backend});
    capture(
        context,
        &source_request,
        &context.source.environment,
        &source_configuration,
        CloneSide::Source,
        phase,
    )?;
    let environment = context.environment()?;
    let inputs = StorageInputs {
        backend: context.backend,
        request: &context.target,
        environment: &environment,
        observation_dir: &context.output,
    };
    let execution = inputs.execution_backend()?;
    let (directory, _) = target_verify::input::request_configuration::fresh_files(
        &execution,
        &environment,
        &context.target,
    )?;
    let target_configuration =
        app_configuration::configuration_value(&execution, &directory, "test")?;
    target(context, &inputs, &target_configuration)?;
    capture(
        context,
        &context.target,
        &environment,
        &target_configuration,
        CloneSide::Target,
        phase,
    )?;
    objects(context, &source_request, &source_configuration, true, phase)?;
    objects(
        context,
        &context.target,
        &target_configuration,
        false,
        phase,
    )?;
    context.check()
}

fn capture(
    context: &Context<'_>,
    request: &Value,
    environment: &BTreeMap<String, String>,
    configuration: &Value,
    side: CloneSide,
    phase: &str,
) -> Result<()> {
    let inputs = StorageInputs {
        backend: context.backend,
        request,
        environment,
        observation_dir: &context.output,
    };
    let captured = target_verify::database::capture_source_at(
        &inputs,
        &context
            .output
            .join(format!("{phase}-{}-inventory", side.as_str())),
        &SystemTransport,
        &|| {
            context.owner.checkpoint()?;
            context.check()
        },
        &SourceBinding {
            side,
            plan: Some(&context.plan),
            selected: &request["target"],
            tools: &request["tools"],
            maintenance: &request["maintenance_build"],
            scope: context.scope,
            configuration: Some(configuration),
        },
    )?;
    for database in inputs.databases()? {
        let key = document::text(&database["key"], "库存逻辑目标")?;
        let actual = captured.observations.get(key).ok_or("完整库存缺少目标")?;
        let expected = if side == CloneSide::Source {
            json!({"tables":actual["tables"],"preserved":actual["preserved"]})
        } else {
            let name = document::text(&database["database"], "目标物理库")?;
            let mut expected = context.initial["inventory"]["observations"][name].clone();
            if !expected.is_object() {
                return Err("原初始化完整库存缺失".into());
            }
            expected["tables"] = context.session["source_observation"][key]["tables"].clone();
            expected
        };
        if side == CloneSide::Source {
            if expected != context.session["source_observation"][key] {
                return Err("原来源完整库存不再等于复制来源".into());
            }
        } else if actual != &expected {
            return Err("准备API前目标完整复制后像变化".into());
        }
    }
    Ok(())
}

fn target(context: &Context<'_>, inputs: &StorageInputs<'_>, configuration: &Value) -> Result<()> {
    let mut declared = inputs.target().clone();
    declared["s3"] = json!({"configuration":configuration});
    let execution = inputs.execution_backend()?;
    if crate::recovery::source::evidence::binding::physical(&json!({"backend":execution,
        "evidence_root":context.backend,"source":declared,"app_env":"test","config_dir":configuration["directory"]}))?
        != context.initial["generation"]["physical"]
    {
        return Err("派生API物理目标不是原复制目标".into());
    }
    let directory = Path::new(document::text(&configuration["directory"], "目标配置目录")?);
    let table = serde_json::to_value(
        crate::rate_limit::load_app_table(&execution, Some(directory), "test")
            .map_err(|_| "已绑定目标配置无法读取；核对TOML字段，不回显正文")?,
    )?;
    export::proof::verify_api_address(
        document::text(&context.selected["api_url"], "目标API地址")?,
        &table,
    )?;
    export::proof::verify_worker_address(
        document::text(&context.selected["worker_ready_url"], "Worker地址")?,
        &table,
    )?;
    for field in ["api_url", "worker_ready_url"] {
        common::require_closed_port(document::text(&context.selected[field], "原目标监听端口")?)?;
    }
    super::files::absent(Path::new(document::text(
        &context.selected["runtime_dir"],
        "原目标运行目录",
    )?))?;
    let (selected, cache) = target_verify::fresh::selection::transition(
        context.backend,
        &context.target,
        Some(context.storage),
        Some(context.owner),
    )?;
    target_verify::storage::protocol::execute(
        context.backend,
        &context.output,
        "storage-layout",
        &json!({"request":context.target_descriptor,"storage":selected.value,"runtime":selected.runtime,"cache_runtime":cache}),
    )?;
    // 新准备运行没有生产者历史；已有任何角色/控制证据都不能通过prepare接管或覆盖。
    if context.post.runtime.try_exists()? {
        for name in [
            "api.json",
            "worker.json",
            "api-tree.json",
            "worker-tree.json",
            "producer-history.json",
            "api-control.lock",
            "worker-control.lock",
        ] {
            super::files::absent(&context.post.runtime.join(name))?;
        }
    }

    Ok(())
}

fn objects(
    context: &Context<'_>,
    request: &Value,
    configuration: &Value,
    source: bool,
    phase: &str,
) -> Result<()> {
    let scope = document::text(&request["target"]["scope_id"], "对象scope")?;
    for bucket in BUCKETS {
        let mut expected = BTreeMap::new();
        let owner = format!("{scope}/.ryframe-owner");
        expected.insert(owner.clone(), None);
        for item in document::array(&context.plan["objects"], "完整复制对象")?
            .iter()
            .filter(|item| item["bucket"] == *bucket)
        {
            let key = document::text(
                &item[if source { "source_key" } else { "target_key" }],
                "复制对象key",
            )?;
            expected.insert(key.to_owned(), Some(item["artifact"].clone()));
        }
        if keys(context, configuration, bucket, scope)?
            != expected.keys().cloned().collect::<BTreeSet<_>>()
        {
            return Err("准备API前完整对象范围不同于复制后像".into());
        }
        for (key, artifact) in expected {
            context.check()?;
            let destination = context.output.join(format!(
                "{phase}-{}-object-{}.bin",
                if source { "source" } else { "target" },
                uuid::Uuid::new_v4().simple()
            ));
            let limit = artifact
                .as_ref()
                .and_then(|v| v["bytes"].as_u64())
                .unwrap_or(4096)
                .max(1);
            observe(
                context,
                configuration,
                bucket,
                Operation::Get {
                    key: key.clone(),
                    if_match: None,
                    version: None,
                    destination: destination.clone(),
                    max_bytes: limit,
                },
            )?;
            let actual = digest::file(&common::common_file(
                context.backend,
                &destination,
                "对象只读正文",
            )?)?;
            if let Some(artifact) = artifact {
                if artifact["bytes"].as_u64() != Some(actual.bytes)
                    || artifact["sha256"] != actual.sha256
                {
                    return Err("准备API前对象GET字节或摘要不同于原复制对象".into());
                }
            } else if std::fs::read(&destination)?
                != format!("ryframe-owner:v1:{scope}:object-storage:{bucket}").as_bytes()
            {
                return Err("对象owner正文不同于固定scope".into());
            }
            context.check()?;
        }
    }
    Ok(())
}

fn keys(
    context: &Context<'_>,
    configuration: &Value,
    bucket: &str,
    scope: &str,
) -> Result<BTreeSet<String>> {
    let mut continuation = None;
    let mut seen = BTreeSet::new();
    let mut keys = BTreeSet::new();
    loop {
        context.check()?;
        let page = observe(
            context,
            configuration,
            bucket,
            Operation::List {
                prefix: format!("{scope}/"),
                continuation,
            },
        )?;
        for entry in document::array(&page["Contents"], "完整对象列表")? {
            let key = document::text(&entry["Key"], "对象key")?;
            if !key.starts_with(&format!("{scope}/")) || !keys.insert(key.to_owned()) {
                return Err("对象分页越界或重复".into());
            }
        }
        if page["IsTruncated"] == false {
            break;
        }
        if page["IsTruncated"] != true {
            return Err("对象分页缺少明确完成状态".into());
        }
        let token = document::text(&page["NextContinuationToken"], "对象下一页")?.to_owned();
        if !seen.insert(token.clone()) {
            return Err("对象分页令牌重复".into());
        }
        continuation = Some(token);
    }
    Ok(keys)
}

fn observe(
    context: &Context<'_>,
    configuration: &Value,
    bucket: &str,
    operation: Operation,
) -> Result<Value> {
    let result = SystemTransport.s3(Request {
        configuration: Configuration::from_app_binding(context.backend, configuration)?,
        bucket: bucket.to_owned(),
        operation,
    });
    let receipt = context.output.join(format!(
        "objects-{}.command.json",
        uuid::Uuid::new_v4().simple()
    ));
    document::write_new(
        &receipt,
        &json!({"transport":"rust-s3-sigv4","bucket":bucket,"remote_writes":0,
        "status":if result.is_ok() {"observed"} else {"failed"},"error_type":result.as_ref().err().map(|_| "NativeS3Error")}),
    )?;
    result
}
