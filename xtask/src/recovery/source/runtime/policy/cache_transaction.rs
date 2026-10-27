//! 固定 33 键同连接 WATCH 比较与一次 EXEC；冲突和未知结果均不自动重试。
use super::{cache_attempt, cache_io, documents};
use crate::{
    Result,
    recovery::clone::native::{
        digest,
        target_verify::{redis::Client, storage::StorageInputs},
    },
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

type Send<'a> = dyn FnMut(&[&str]) -> Result<Value> + 'a;

pub(crate) fn cleanup(args: &Value) -> Result<Value> {
    cleanup_with(
        args,
        |inputs, redis, comparison| match Client::authenticated(inputs, redis) {
            Ok(mut client) => execute(comparison, &mut |parts| client.transaction_command(parts)),
            Err(_) => outcome("pre_exec_io_failed"),
        },
    )
}

// 只替换连接执行端口；比较、独占 attempt 及执行结果持久化仍走同一正式生命周期。
pub(super) fn cleanup_with(
    args: &Value,
    run: impl FnOnce(&StorageInputs<'_>, &Value, &cache_io::Comparison) -> Value,
) -> Result<Value> {
    documents::exact(
        args,
        &[
            "backend",
            "directory",
            "output",
            "request",
            "environment",
            "observation",
            "comparisons",
        ],
        "来源缓存事务参数",
    )?;
    let comparison = cache_io::comparison(
        &json!({"observation":args["observation"],"comparisons":args["comparisons"]}),
    )?;
    let backend = PathBuf::from(documents::text(&args["backend"], "来源后端")?);
    if !backend.is_absolute() || !backend.is_dir() || digest::linked_path(&backend)?.contains(&true)
    {
        return Err("来源缓存后端必须是绝对普通目录；核对已绑定工作树".into());
    }
    documents::exact(&args["environment"], &["APP_ENV"], "来源缓存配置选择")?;
    if args["environment"]["APP_ENV"] != "test" {
        return Err("来源缓存必须使用已绑定 test 配置；核对当前来源登记".into());
    }
    let scope = documents::text(&args["request"]["target"]["scope_id"], "来源缓存 scope")?;
    if args["observation"]["namespace"] != format!("ryframe:{{{scope}}}:") {
        return Err("来源缓存 namespace 不属于绑定目标；重新核对完整前像".into());
    }
    let frame = cache_attempt::Frame::open(args)?;
    if frame.request != args["request"] {
        return Err("缓存写请求不同于原源代次 seed 请求；核对原登记，禁止覆盖".into());
    }
    let attempt = frame.create(&args["observation"])?;
    let environment = BTreeMap::from([("APP_ENV".into(), "test".into())]);
    let inputs = StorageInputs {
        backend: &backend,
        request: &args["request"],
        environment: &environment,
        observation_dir: &backend,
    };
    let result = run(&inputs, &args["request"]["storage"]["redis"], &comparison);
    if frame.finish(&attempt, &result).is_err() {
        return Ok(outcome("unknown_exec_result"));
    }
    Ok(result)
}

fn outcome(status: &str) -> Value {
    let (reason, may_write) = match status {
        "confirmed" => ("固定 33 键清理已确认；保留绑定收据并核对完整后像", true),
        "precondition_failed" => (
            "WATCH 后完整类型或值与冻结前像不同，未发送 EXEC；保留现场并核对绑定配置与完整前像，不重试",
            false,
        ),
        "watch_conflict" => (
            "WATCH 检测到竞争或 TTL/ABA 变化，EXEC 已中止且未执行 UNLINK；保留现场人工核对，不重试",
            false,
        ),
        "pre_exec_io_failed" => (
            "EXEC 前连接、认证或响应未确认，未发送 EXEC；检查绑定文件配置及服务连接，保留原 attempt，不重试",
            false,
        ),
        "unsupported_watch_expiration" => (
            "Redis 版本不能证明 WATCH 过期语义；需使用 6.0.9 或更新版本，保留原 attempt 并核对现场，不重试",
            false,
        ),
        _ => (
            "EXEC 或执行结果持久化未确认；按原 attempt/result 核对完整前后像，禁止推断零写入或重放",
            true,
        ),
    };
    json!({"status":status,"deleted":null,"expired":[],"message":reason,
        "remote_write_may_have_occurred":may_write,"automatic_retry_permitted":false})
}

#[derive(Clone, Copy)]
enum Before {
    Communication,
    Mismatch,
    Unsupported,
}

fn expect(
    send: &mut Send<'_>,
    parts: &[&str],
    expected: &Value,
) -> std::result::Result<(), Before> {
    if send(parts).map_err(|_| Before::Communication)? != *expected {
        return Err(Before::Mismatch);
    }
    Ok(())
}

fn version(send: &mut Send<'_>) -> std::result::Result<(), Before> {
    let info = send(&["INFO", "server"]).map_err(|_| Before::Communication)?;
    let text = info.as_str().ok_or(Before::Unsupported)?;
    let versions = text
        .lines()
        .filter_map(|line| line.strip_prefix("redis_version:"))
        .collect::<Vec<_>>();
    if versions.len() != 1 {
        return Err(Before::Unsupported);
    }
    let fields = versions[0]
        .trim_end_matches('\r')
        .split('.')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| Before::Unsupported)?;
    if fields.len() != 3 || fields.as_slice() < [6_u32, 0, 9].as_slice() {
        return Err(Before::Unsupported);
    }
    Ok(())
}

fn prepare(plan: &cache_io::Comparison, send: &mut Send<'_>) -> std::result::Result<(), Before> {
    version(send)?;
    let mut watch = vec!["WATCH"];
    watch.extend(plan.keys.iter().map(String::as_str));
    expect(send, &watch, &json!("OK"))?;
    for (keys, values) in plan
        .keys
        .as_chunks::<3>()
        .0
        .iter()
        .zip(plan.values.as_chunks::<5>().0)
    {
        expect(send, &["TYPE", &keys[0]], &json!("string"))?;
        expect(send, &["GET", &keys[0]], &json!(values[0]))?;
        expect(send, &["TYPE", &keys[1]], &json!("string"))?;
        expect(send, &["GET", &keys[1]], &json!(values[1]))?;
        expect(send, &["TYPE", &keys[2]], &json!(values[2]))?;
        if values[2] == "hash" {
            expect(send, &["HLEN", &keys[2]], &json!(1))?;
            expect(send, &["HGET", &keys[2], &values[3]], &json!(values[4]))?;
        }
    }
    expect(send, &["MULTI"], &json!("OK"))?;
    let mut unlink = vec!["UNLINK"];
    unlink.extend(plan.keys.iter().map(String::as_str));
    expect(send, &unlink, &json!("QUEUED"))
}

pub(super) fn execute(plan: &cache_io::Comparison, send: &mut Send<'_>) -> Value {
    if let Err(error) = prepare(plan, send) {
        let status = match error {
            Before::Communication => "pre_exec_io_failed",
            Before::Mismatch => "precondition_failed",
            Before::Unsupported => "unsupported_watch_expiration",
        };
        return outcome(status);
    }
    // 发送 EXEC 后任何通信或协议异常均保守标记未知，不重连、不重发。
    match send(&["EXEC"]) {
        Ok(Value::Null) => outcome("watch_conflict"),
        Ok(Value::Array(values))
            if values.len() == 1 && values[0].as_u64() == Some(33 - plan.expired.len() as u64) =>
        {
            let mut result = outcome("confirmed");
            result["deleted"] = values[0].clone();
            result["expired"] = json!(plan.expired);
            result
        }
        _ => outcome("unknown_exec_result"),
    }
}
