use super::{cache_io, cache_transaction, operations_tests::cache_fixture};
use crate::Result;
use serde_json::{Value, json};

struct OrdinaryTransport(std::cell::RefCell<Vec<&'static str>>);

impl crate::recovery::clone::native::export::transport::Transport for OrdinaryTransport {
    fn run(
        &self,
        _: crate::recovery::clone::native::export::transport::CommandSpec,
    ) -> Result<crate::recovery::clone::native::export::transport::CommandOutput> {
        self.0.borrow_mut().push("command");
        Err("普通测试端口不得执行命令".into())
    }

    fn s3(&self, _: crate::recovery::clone::native::s3::Request) -> Result<Value> {
        self.0.borrow_mut().push("s3");
        Err("普通测试端口不得访问 S3".into())
    }

    fn publish_evidence(&self, _: &std::path::Path, _: &[u8]) -> std::io::Result<()> {
        self.0.borrow_mut().push("evidence");
        Err(std::io::Error::other("普通测试端口不得发布证据"))
    }
}

#[test]
fn ordinary_transport_cleanup_rejects_without_resource_calls_or_attempt_publication() {
    use crate::recovery::source::evidence::resources::Resources;
    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("source-cleanup-default-port-")
        .tempdir_in(local)
        .unwrap();
    let output = directory.path().join("source-runtime.json");
    let transport = OrdinaryTransport(std::cell::RefCell::new(Vec::new()));
    let environment = std::collections::BTreeMap::from([("APP_ENV".into(), "test".into())]);
    let request = json!({"target":{"scope_id":"source-unit"}});
    let (observation, data) = cache_fixture();
    let resources = Resources {
        backend: &backend,
        execution: &backend,
        request: &request,
        generation: &Value::Null,
        environment: &environment,
        output: &output,
        transport: &transport,
        checkpoint: &|| Ok(()),
    };
    // 仅验证写端口拒绝，不调用父链只读 Redis 观察，也不伪造有效 START。
    let error = resources
        .cleanup(&Value::Null, &observation, &data["comparisons"])
        .unwrap_err();
    assert!(error.to_string().contains("当前资源端口未授权来源缓存清理"));
    assert!(transport.0.borrow().is_empty());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

fn plan() -> cache_io::Comparison {
    let (observation, data) = cache_fixture();
    cache_io::comparison(&json!({"observation":observation,"comparisons":data["comparisons"]}))
        .unwrap()
}

// 只提供固定 Redis 响应，不模拟 WATCH 的服务端竞争算法。
fn reply(plan: &cache_io::Comparison, parts: &[&str]) -> Value {
    match parts[0] {
        "INFO" => return json!("redis_version:7.4.0\r\n"),
        "WATCH" | "MULTI" => return json!("OK"),
        "UNLINK" => return json!("QUEUED"),
        _ => {}
    }
    let index = plan.keys.iter().position(|key| key == parts[1]).unwrap();
    let values = &plan.values[index / 3 * 5..index / 3 * 5 + 5];
    match parts[0] {
        "TYPE" => json!(if index % 3 == 2 { &values[2] } else { "string" }),
        "GET" => json!(values[index % 3]),
        "HLEN" => json!(1),
        "HGET" => json!(values[4]),
        _ => panic!("unexpected command kind"),
    }
}

#[test]
fn peer_drift_before_or_after_exec_preserves_distinct_write_outcomes() {
    let plan = plan();
    for (failure_check, status, exec_count) in
        [(1, "pre_exec_io_failed", 0), (2, "unknown_exec_result", 1)]
    {
        let checks = std::cell::Cell::new(0);
        let mut sent = 0;
        let result = cache_transaction::execute_on(
            &plan,
            &mut sent,
            |sent, parts| {
                if parts[0] == "EXEC" {
                    *sent += 1;
                    Ok(json!([33 - plan.expired.len()]))
                } else {
                    Ok(reply(&plan, parts))
                }
            },
            |_| {
                checks.set(checks.get() + 1);
                if checks.get() == failure_check {
                    Err("Redis peer drift".into())
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result["status"], status);
        assert_eq!(result["remote_write_may_have_occurred"], exec_count == 1);
        assert_eq!(result["automatic_retry_permitted"], false);
        assert_eq!(sent, exec_count);
    }
}

#[test]
fn confirmed_cleanup_watches_all_keys_before_reading_and_queues_one_unlink() {
    let plan = plan();
    let mut commands = Vec::new();
    let result = cache_transaction::execute(&plan, &mut |parts| {
        commands.push(
            parts
                .iter()
                .map(|part| part.to_string())
                .collect::<Vec<_>>(),
        );
        Ok(if parts[0] == "EXEC" {
            json!([33 - plan.expired.len()])
        } else {
            reply(&plan, parts)
        })
    });
    assert_eq!(result["status"], "confirmed");
    assert_eq!(result["deleted"], 33 - plan.expired.len());
    assert_eq!(result["expired"], json!(plan.expired));
    assert_eq!(commands[1][0], "WATCH");
    assert_eq!(&commands[1][1..], plan.keys.as_slice());
    let unlink = commands
        .iter()
        .filter(|parts| parts[0] == "UNLINK")
        .collect::<Vec<_>>();
    assert_eq!(unlink.len(), 1);
    assert_eq!(&unlink[0][1..], plan.keys.as_slice());
    assert_eq!(
        commands.iter().filter(|parts| parts[0] == "EXEC").count(),
        1
    );
    assert_eq!(commands[commands.len() - 3][0], "MULTI");
    assert_eq!(result["automatic_retry_permitted"], false);
}

#[test]
fn changed_values_or_types_reject_before_multi_and_do_not_expose_responses() {
    let plan = plan();
    for command in ["TYPE", "GET", "HLEN", "HGET"] {
        let mut commands = Vec::new();
        let result = cache_transaction::execute(&plan, &mut |parts| {
            commands.push(parts[0].to_string());
            Ok(if parts[0] == command {
                json!("private-response-sentinel")
            } else {
                reply(&plan, parts)
            })
        });
        assert_eq!(result["status"], "precondition_failed");
        assert_eq!(result["remote_write_may_have_occurred"], false);
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command.as_str(), "MULTI" | "UNLINK" | "EXEC"))
        );
        assert!(!result.to_string().contains("private-response-sentinel"));
        assert!(result["message"].as_str().unwrap().contains("未发送 EXEC"));
        assert!(result["message"].as_str().unwrap().contains("核对"));
    }
}

#[test]
fn watched_conflict_aborts_once_and_lost_or_malformed_exec_is_unknown_without_retry() {
    let plan = plan();
    for response in [
        Some(Value::Null),
        None,
        Some(json!(["33"])),
        Some(json!([true])),
        Some(json!([-1])),
        Some(json!([33, 0])),
    ] {
        let mut execs = 0;
        let mut unlinks = 0;
        let result = cache_transaction::execute(&plan, &mut |parts| -> Result<Value> {
            if parts[0] == "UNLINK" {
                unlinks += 1;
            }
            if parts[0] == "EXEC" {
                execs += 1;
                return response
                    .clone()
                    .ok_or_else(|| "private-transport-sentinel".into());
            }
            Ok(reply(&plan, parts))
        });
        assert_eq!(execs, 1);
        assert_eq!(unlinks, 1);
        assert_eq!(result["deleted"], Value::Null);
        let conflict = response == Some(Value::Null);
        assert_eq!(
            result["status"],
            if conflict {
                "watch_conflict"
            } else {
                "unknown_exec_result"
            }
        );
        assert_eq!(result["remote_write_may_have_occurred"], !conflict);
        assert_eq!(result["automatic_retry_permitted"], false);
        assert!(!result.to_string().contains("private-transport-sentinel"));
        let message = result["message"].as_str().unwrap();
        assert!(message.contains("核对"));
        assert!(message.contains(if conflict {
            "未执行 UNLINK"
        } else {
            "禁止推断零写入或重放"
        }));
    }
}

#[test]
fn read_failure_and_unsupported_expiration_semantics_never_send_exec() {
    let plan = plan();
    for old_version in [false, true] {
        let mut commands = Vec::new();
        let result = cache_transaction::execute(&plan, &mut |parts| -> Result<Value> {
            commands.push(parts[0].to_owned());
            if parts[0] == "INFO" && old_version {
                return Ok(json!("redis_version:6.0.8\r\n"));
            }
            if parts[0] == "GET" {
                return Err("private-sentinel".into());
            }
            Ok(reply(&plan, parts))
        });
        assert_eq!(
            result["status"],
            if old_version {
                "unsupported_watch_expiration"
            } else {
                "pre_exec_io_failed"
            }
        );
        assert_eq!(result["remote_write_may_have_occurred"], false);
        assert!(!commands.iter().any(|command| command == "EXEC"));
        let message = result["message"].as_str().unwrap();
        assert!(message.contains(if old_version {
            "6.0.9"
        } else {
            "检查绑定文件配置"
        }));
        assert!(!message.contains("private-sentinel"));
    }
}
