use std::{cell::RefCell, collections::BTreeMap};

use serde_json::{Value, json};

use crate::recovery::clone::native::target_verify::{
    input::TargetContext, redis, storage::StorageObserver,
};

const PREFIX: &str = "ryframe:{state-fixture}:";
const OWNER: &str = "ryframe:{state-fixture}:.ryframe-owner";
const SENTINEL: &str = "ryframe:devex-fresh:state-fixture:sentinel";

struct FakeObserver {
    selected: Value,
    replies: BTreeMap<Vec<String>, Value>,
    calls: RefCell<Vec<Vec<String>>>,
}

impl StorageObserver for FakeObserver {
    fn process_identity(&self, _pid: u32) -> crate::Result<Option<Value>> {
        Err("Redis 状态测试不应读取原生进程".into())
    }

    fn arguments(&self, _context: &TargetContext, _identity: &Value) -> crate::Result<Vec<String>> {
        Err("Redis 状态测试不应读取原生参数".into())
    }

    fn listener(
        &self,
        _context: &TargetContext,
        _identity: &Value,
        _url: &str,
    ) -> crate::Result<()> {
        Err("Redis 状态测试不应读取原生监听".into())
    }

    fn wsl(
        &self,
        _context: &TargetContext,
        _redis: &Value,
        _args: &[String],
    ) -> crate::Result<String> {
        Err("Redis 状态测试不应执行 WSL".into())
    }

    fn redis(
        &self,
        _context: &TargetContext,
        selected: &Value,
        parts: &[&str],
    ) -> crate::Result<Value> {
        assert_eq!(
            selected, &self.selected,
            "必须使用调用方明确选定的 Redis 代次"
        );
        let key = parts
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>();
        self.calls.borrow_mut().push(key.clone());
        self.replies
            .get(&key)
            .cloned()
            .ok_or_else(|| "未登记的 Redis 观察命令".into())
    }
}

fn scan(cursor: &str) -> Vec<String> {
    [
        "SCAN",
        cursor,
        "MATCH",
        &format!("{PREFIX}*"),
        "COUNT",
        "1000",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect()
}

fn fixture() -> (TargetContext, Value, FakeObserver) {
    let backend = crate::workspace::root_dir();
    let root = backend.join(".local-tests/redis-state-observer");
    let mut context = super::target_storage_model_tests::context(&backend, &root, &root);
    context.request["target"]["scope_id"] = json!("state-fixture");
    context.request["reset"] = json!({"sentinel_key":SENTINEL,"sentinel_value":"fixed-sentinel"});
    let selected = json!({"pid":200,"run_id":"3".repeat(40),"port":16390});
    let replies = BTreeMap::from([
        (scan("0"), json!(["0", [OWNER]])),
        (
            vec!["GET".into(), OWNER.into()],
            json!("ryframe-owner:v1:state-fixture:redis"),
        ),
        (vec!["GET".into(), SENTINEL.into()], json!("fixed-sentinel")),
        (
            vec!["INFO".into(), "server".into()],
            json!(format!("process_id:200\r\nrun_id:{}\r\n", "3".repeat(40))),
        ),
    ]);
    let observer = FakeObserver {
        selected: selected.clone(),
        replies,
        calls: RefCell::new(Vec::new()),
    };
    (context, selected, observer)
}

#[test]
fn redis_state_uses_explicit_observer_and_exact_scope_without_sockets() {
    let (context, selected, observer) = fixture();
    let observed = redis::verify_with_observer(&context, &selected, &observer).unwrap();
    assert_eq!(
        observed,
        json!({"keys":[OWNER],"owner":"ryframe-owner:v1:state-fixture:redis","sentinel":"fixed-sentinel"})
    );
    assert_eq!(
        observer.calls.borrow().as_slice(),
        &[
            scan("0"),
            vec!["GET".into(), OWNER.into()],
            vec!["GET".into(), SENTINEL.into()],
            vec!["INFO".into(), "server".into()],
        ]
    );
}

#[test]
fn redis_state_rejects_owner_sentinel_unknown_keys_and_info_identity_drift() {
    for (command, response) in [
        (vec!["GET".into(), OWNER.into()], Value::Null),
        (
            vec!["GET".into(), SENTINEL.into()],
            json!("changed-sentinel"),
        ),
        (scan("0"), json!(["0", [OWNER, format!("{PREFIX}unknown")]])),
        (
            vec!["INFO".into(), "server".into()],
            json!(format!("process_id:201\nrun_id:{}\n", "3".repeat(40))),
        ),
        (
            vec!["INFO".into(), "server".into()],
            json!(format!("process_id:200\nrun_id:{}\n", "4".repeat(40))),
        ),
        (
            vec!["INFO".into(), "server".into()],
            json!(format!(
                "process_id:200\nprocess_id:200\nrun_id:{}\n",
                "3".repeat(40)
            )),
        ),
    ] {
        let (context, selected, mut observer) = fixture();
        observer.replies.insert(command, response);
        assert!(redis::verify_with_observer(&context, &selected, &observer).is_err());
    }
}

#[test]
fn redis_state_rejects_scan_escape_duplicates_and_malformed_pages_before_get() {
    for page in [
        json!(["0", ["other-scope:key"]]),
        json!(["0", [OWNER, OWNER]]),
        json!(["", []]),
        json!(["bad", []]),
        json!([0, []]),
        json!(["0"]),
        json!(["0", {}]),
    ] {
        let (context, selected, mut observer) = fixture();
        observer.replies.insert(scan("0"), page);
        assert!(redis::verify_with_observer(&context, &selected, &observer).is_err());
        assert_eq!(observer.calls.borrow().as_slice(), &[scan("0")]);
    }
}

#[test]
fn redis_state_rejects_repeated_cursor_without_unbounded_observation() {
    let (context, selected, mut observer) = fixture();
    observer.replies.insert(scan("0"), json!(["7", []]));
    observer.replies.insert(scan("7"), json!(["7", []]));
    let error = redis::verify_with_observer(&context, &selected, &observer).unwrap_err();
    assert!(error.to_string().contains("游标循环"));
    assert_eq!(observer.calls.borrow().as_slice(), &[scan("0"), scan("7")]);
}
