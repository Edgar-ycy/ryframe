//! 生成 arm 测试所需的原始和当前存储绑定。
use std::{fs, path::Path};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::recovery::clone::native::lifecycle::state;

pub fn arm_storage_bindings(storage_run: &Path) -> (Value, Value) {
    fs::create_dir(storage_run).unwrap();
    let manifest = storage_run.join("manifest.json");
    write_json(&manifest, &json!({"kind":"devex-clone-run","fixture":true}));
    state::initialize(storage_run).unwrap();
    let storage_attempt = state::begin(
        storage_run,
        "storage-target",
        "restart",
        &json!({"fixture":true}),
    )
    .unwrap()
    .0;
    state::finish(
        storage_run,
        storage_attempt,
        Some(&json!({"status":"storage_restarted"})),
        None,
    )
    .unwrap();
    let historical = json!({
        "path": storage_run,
        "manifest": binding(&manifest),
        "state": binding(&storage_run.join("state.json")),
    });
    let cache_attempt = state::begin(
        storage_run,
        "cache-target",
        "restart",
        &json!({"fixture":true}),
    )
    .unwrap()
    .0;
    state::finish(
        storage_run,
        cache_attempt,
        Some(&json!({"status":"cache_restarted"})),
        None,
    )
    .unwrap();
    let evidence = (0..8)
        .map(|index| {
            let path = storage_run.join(format!("lifecycle-{index}.json"));
            write_json(&path, &json!({"fixture":index}));
            binding(&path)
        })
        .collect::<Vec<_>>();
    let effective = json!({
        "path": storage_run,
        "manifest": binding(&manifest),
        "storage": {
            "request": evidence[0],
            "storage": {
                "identity": {"pid": 1},
                "sha256": "a".repeat(64),
                "process_receipt": evidence[1],
                "launch_receipt": evidence[2],
            },
            "attempt": storage_attempt,
            "data_directory": storage_run.join("data"),
            "api_url": "http://127.0.0.1:1",
            "console_url": "http://127.0.0.1:2",
            "restart_result": binding(&state::result_path(storage_run, storage_attempt)),
        },
        "cache": {
            "request": evidence[3],
            "runtime": evidence[4],
            "redis": {"pid": 2},
            "owner": evidence[5],
            "controller": evidence[6],
            "attempt": cache_attempt,
            "ready": evidence[7],
            "restart_result": binding(&state::result_path(storage_run, cache_attempt)),
        },
    });
    (historical, effective)
}

pub fn binding(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap();
    json!({
        "path": path.to_string_lossy(),
        "bytes": bytes.len(),
        "sha256": hex::encode(Sha256::digest(&bytes)),
    })
}

pub fn write_json(path: &Path, value: &Value) {
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    fs::write(path, text).unwrap();
}
