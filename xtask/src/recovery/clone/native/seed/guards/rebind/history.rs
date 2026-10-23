use super::{attempts, current, operation, reject};
use crate::{Result, recovery::clone::native::json as document};
use serde_json::{Value, json};

pub(super) fn read_only(op: (&str, &str)) -> bool {
    matches!(
        op,
        (
            "seed-runtime",
            "arm-input"
                | "source-generation-start"
                | "source-generation-stop"
                | "source-generation-recover"
                | "source-export"
                | "source-export-reconcile"
        ) | ("storage-target", "status")
    )
}

fn before_rebind(op: (&str, &str)) -> bool {
    matches!(
        op,
        ("seed-runtime", "arm-input" | "stop" | "recover")
            | ("storage-target", "status" | "restart" | "stop" | "recover")
    )
}

/// 先验证整个状态机，再由采集器依序完成计划中的只读证明。
pub(super) fn plan(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "attempts",
            "descriptor",
            "current",
            "archive",
            "segment",
            "pending",
            "backend_available",
            "cache_recovery",
            "generation_output_exists",
        ],
        "重绑定历史计划",
    )?;
    let rows = attempts(&args["attempts"])?;
    let number = current(&args["current"])?;
    let available = document::boolean(&args["backend_available"], "缓存证明工具")?;
    let recovery = document::boolean(&args["cache_recovery"], "缓存恢复")?;
    let output_exists = document::boolean(&args["generation_output_exists"], "代次输出")?;
    let published: Vec<_> = rows
        .iter()
        .filter(|row| {
            operation(row) == ("seed-runtime", "source-register")
                && row["result"] == args["descriptor"]
                && row["status"] == "passed"
        })
        .collect();
    if published.len() != 1 {
        return Ok(reject("重绑定必须继承唯一已发布 seed 源"));
    }
    let later: Vec<_> = rows
        .iter()
        .filter(|row| row["number"].as_u64() > published[0]["number"].as_u64())
        .collect();
    let archive = &args["archive"];
    if !archive.is_null() && archive["receipt"]["source_registration"] != args["descriptor"] {
        return Ok(reject("未启动恢复没有绑定当前已发布 seed 源"));
    }
    let archived = records(archive)?;
    let segmented = records(&args["segment"])?;
    let pending = document::array(&args["pending"], "未启动失败前缀")?;
    if !pending.is_empty() && (!archive.is_null() || !pending.iter().all(|row| rows.contains(row)))
    {
        return Err("未启动恢复证明不属于当前历史".into());
    }
    let mut machine = Machine::default();
    let mut actions = Vec::new();
    let context = Context {
        later: &later,
        rows,
        number,
        available,
        recovery,
        output_exists,
        pending,
    };
    for row in &later {
        if archived.contains(row)
            || segmented.contains(row)
            || pending.iter().skip(1).any(|p| p == *row)
        {
            continue;
        }
        if let Some(error) = machine.step(row, &context, &mut actions)? {
            return Ok(error);
        }
    }
    Ok(json!({"later":later, "actions":actions}))
}

fn records(value: &Value) -> Result<&[Value]> {
    if value.is_null() {
        return Ok(&[]);
    }
    document::array(&value["records"], "封存或分段阶段")
}

#[derive(Default)]
struct Machine {
    rebound: bool,
    started: bool,
    generated: bool,
    recovered: bool,
    exported: bool,
    cache: bool,
    export_origin: Option<Value>,
}

struct Context<'a> {
    later: &'a [&'a Value],
    rows: &'a [Value],
    number: Option<u64>,
    available: bool,
    recovery: bool,
    output_exists: bool,
    pending: &'a [Value],
}

impl Machine {
    fn step(
        &mut self,
        row: &Value,
        context: &Context<'_>,
        actions: &mut Vec<Value>,
    ) -> Result<Option<Value>> {
        let op = operation(row);
        let active = row["number"].as_u64() == context.number;
        if row["stage"] == "cache-target" {
            return self.cache_step(row, context, actions);
        }
        if op == ("seed-runtime", "source-rebind") && self.cache {
            actions.push(json!({"kind":"registered-cache"}));
        }
        if let Some(error) = self.advance(op, row, context.later) {
            return Ok(Some(error));
        }
        if active {
            if context.rows.last() != Some(row)
                || row["status"] != "running"
                || !(if self.rebound {
                    read_only(op)
                } else {
                    before_rebind(op) || op == ("seed-runtime", "source-rebind")
                })
            {
                return Ok(Some(reject("重绑定核验只能排除当前持锁阶段")));
            }
            actions.push(json!({"kind":"owner"}));
            return Ok(None);
        }
        if op == ("seed-runtime", "source-rebind") {
            if self.rebound || row["status"] != "passed" {
                return Ok(Some(reject("seed 源存在重复、冲突或未收尾重绑定")));
            }
            self.rebound = true;
            if self.cache {
                actions.push(json!({"kind":"rebind-cache", "record":row}));
            }
        } else if !(if self.rebound {
            read_only(op)
        } else {
            before_rebind(op)
        }) {
            return Ok(Some(reject(
                "seed 发布后出现未知写入或重绑定后存储再次换代",
            )));
        }
        Ok(self.failure(row, context, actions))
    }

    fn cache_step(
        &mut self,
        row: &Value,
        context: &Context<'_>,
        actions: &mut Vec<Value>,
    ) -> Result<Option<Value>> {
        if self.rebound
            || !context.available
            || !matches!(
                operation(row).1,
                "restart" | "resume" | "reconcile" | "stop" | "recover"
            )
        {
            return Ok(Some(reject("seed 重绑定后或未经验证的缓存操作不可消费")));
        }
        self.cache = true;
        let active = row["number"].as_u64() == context.number;
        if active && (context.rows.last() != Some(row) || row["status"] != "running") {
            return Ok(Some(reject("缓存工具后继不是当前唯一持锁阶段")));
        }
        if !active && row["status"] != "passed" && !context.recovery {
            let latest = context.later.iter().rfind(|next| {
                next["number"].as_u64() > row["number"].as_u64() && next["stage"] == "cache-target"
            });
            if latest
                .is_none_or(|r| r["number"].as_u64() != context.number && r["status"] != "passed")
            {
                return Ok(Some(reject("seed 缓存仍有未核对的失败阶段")));
            }
        }
        actions.push(json!({"kind":"cache", "record":row, "owner":active}));
        Ok(None)
    }

    fn failure(
        &self,
        row: &Value,
        context: &Context<'_>,
        actions: &mut Vec<Value>,
    ) -> Option<Value> {
        let op = operation(row);
        if row["status"] == "passed" || context.pending.first() == Some(row) {
            return None;
        }
        if op == ("seed-runtime", "source-generation-start")
            && context.rows.last() == Some(row)
            && !context.output_exists
        {
            actions.push(json!({"kind":"zero-output", "record":row}));
            return None;
        }
        if matches!(
            op,
            (
                "seed-runtime",
                "source-generation-start" | "source-generation-stop"
            )
        ) {
            let recovered: Vec<_> = context
                .later
                .iter()
                .filter(|next| {
                    next["number"].as_u64() > row["number"].as_u64()
                        && operation(next) == ("seed-runtime", "source-generation-recover")
                })
                .collect();
            if recovered.len() == 1 && recovered[0]["number"].as_u64() == context.number {
                return None;
            }
        }
        if matches!(
            op,
            ("seed-runtime", "source-export" | "source-export-reconcile")
        ) && let Some(origin) = &self.export_origin
        {
            actions.push(json!({"kind":"export-reconcile", "origin":origin}));
            return None;
        }
        Some(reject("seed 发布后存在失败或未收尾阶段"))
    }

    fn advance(&mut self, op: (&str, &str), row: &Value, later: &[&Value]) -> Option<Value> {
        match op {
            ("seed-runtime", "source-generation-start") => {
                if !self.rebound || self.started || self.exported {
                    return Some(reject("source-generation-start 只能在唯一重绑定后执行一次"));
                }
                self.started = true;
            }
            ("seed-runtime", "source-generation-stop") => {
                if !self.started || self.generated || self.recovered || self.exported {
                    return Some(reject(
                        "source-generation-stop 只能消费唯一启动代次并发布一次",
                    ));
                }
                self.generated = true;
            }
            ("seed-runtime", "source-generation-recover") => {
                let completed = later.iter().any(|next| {
                    next["number"].as_u64() < row["number"].as_u64()
                        && operation(next) == ("seed-runtime", "source-generation-stop")
                        && next["status"] == "passed"
                });
                if !self.started || self.recovered || self.exported || completed {
                    return Some(reject(
                        "source-generation-recover 只能收回已有代次，不能重放或改变已导出来源",
                    ));
                }
                self.recovered = true;
            }
            ("seed-runtime", "source-export") => {
                if !self.rebound || !self.generated || self.recovered || self.exported {
                    return Some(reject(
                        "source-export 只允许在后继源代次发布后执行并发布一次",
                    ));
                }
                self.exported = true;
                self.export_origin = Some(row.clone());
            }
            ("seed-runtime", "source-export-reconcile") if !self.exported => {
                return Some(reject("source-export-reconcile 必须继承原导出阶段"));
            }
            _ => {}
        }
        None
    }
}

pub(super) fn proofs(args: &Value) -> Result<Value> {
    document::exact(args, &["proofs"], "重绑定历史证明")?;
    let mut successor = None;
    for proof in document::array(&args["proofs"], "缓存及交接证明")? {
        document::exact(proof, &["kind", "successor"], "缓存或重绑定证明")?;
        if !matches!(proof["kind"].as_str(), Some("cache" | "rebind-cache")) {
            return Err("未知历史证明".into());
        }
        if let Some(before) = successor {
            if before != &proof["successor"] {
                return Ok(reject(if proof["kind"] == "cache" {
                    "seed 缓存历史混入不同 successor"
                } else {
                    "source-rebind 没有消费同一缓存工具 successor"
                }));
            }
        } else if proof["kind"] != "cache" {
            return Err("重绑定缓存证明缺少前驱".into());
        }
        successor = Some(&proof["successor"]);
    }
    Ok(json!({"verified":true}))
}
