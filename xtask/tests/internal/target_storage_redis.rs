#![cfg(windows)]

use std::{cell::RefCell, collections::BTreeMap, fs};

use serde_json::{Value, json};

use crate::recovery::clone::native::{
    resources::common,
    target_verify::{
        input::TargetContext,
        storage::{StorageObserver, redis_layout},
    },
};

struct FakeObserver {
    wsl: BTreeMap<Vec<String>, String>,
    redis: BTreeMap<Vec<String>, Value>,
    calls: RefCell<Vec<Vec<String>>>,
}

impl StorageObserver for FakeObserver {
    fn process_identity(&self, _pid: u32) -> crate::Result<Option<Value>> {
        Err("Redis 布局测试不应读取 RustFS 进程".into())
    }

    fn arguments(&self, _context: &TargetContext, _identity: &Value) -> crate::Result<Vec<String>> {
        Err("Redis 布局测试不应读取 RustFS 参数".into())
    }

    fn listener(
        &self,
        _context: &TargetContext,
        _identity: &Value,
        _url: &str,
    ) -> crate::Result<()> {
        Err("Redis 布局测试不应读取 RustFS 监听".into())
    }

    fn wsl(
        &self,
        _context: &TargetContext,
        _redis: &Value,
        args: &[String],
    ) -> crate::Result<String> {
        self.calls.borrow_mut().push(args.to_vec());
        self.wsl
            .get(args)
            .cloned()
            .ok_or_else(|| "测试调用了未安排的 WSL 命令".into())
    }

    fn redis(
        &self,
        _context: &TargetContext,
        _redis: &Value,
        parts: &[&str],
    ) -> crate::Result<Value> {
        let key = parts
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>();
        self.calls.borrow_mut().push(key.clone());
        self.redis
            .get(&key)
            .cloned()
            .ok_or_else(|| "测试调用了未安排的 Redis 命令".into())
    }
}

fn observer(config: &Value, directory: &str) -> FakeObserver {
    let mut redis = BTreeMap::new();
    let linux_file = format!("{directory}/redis.conf");
    redis.insert(
        vec!["INFO".into(), "server".into()],
        json!(format!(
            "process_id:200\r\nrun_id:{}\r\nconfig_file:{linux_file}\r\n",
            "3".repeat(40)
        )),
    );
    for (key, value) in [
        ("bind", "127.0.0.1"),
        ("port", "16390"),
        ("dir", directory),
        ("databases", "1"),
        ("protected-mode", "yes"),
        ("save", ""),
        ("appendonly", "no"),
        ("daemonize", "no"),
    ] {
        redis.insert(
            vec!["CONFIG".into(), "GET".into(), key.into()],
            json!([key, value]),
        );
    }
    let stat = format!("200 (redis-server) {} 67890", vec!["0"; 19].join(" "));
    let wsl = BTreeMap::from([
        (vec!["/usr/bin/cat".into(), "/proc/200/stat".into()], stat),
        (
            vec![
                "/usr/bin/readlink".into(),
                "-f".into(),
                "/proc/200/exe".into(),
            ],
            "/usr/bin/redis-server".into(),
        ),
        (
            vec!["/usr/bin/sha256sum".into(), "/usr/bin/redis-server".into()],
            format!("{}  /usr/bin/redis-server", "9".repeat(64)),
        ),
        (
            vec!["/usr/bin/readlink".into(), "-f".into(), linux_file.clone()],
            linux_file.clone(),
        ),
        (
            vec!["/usr/bin/sha256sum".into(), linux_file.clone()],
            format!("{}  {linux_file}", config["sha256"].as_str().unwrap()),
        ),
    ]);
    FakeObserver {
        wsl,
        redis,
        calls: RefCell::new(Vec::new()),
    }
}

#[cfg(windows)]
#[test]
fn redis_fake_observer_keeps_real_file_hash_path_and_tool_validation() {
    let backend = crate::workspace::root_dir();
    let parent = backend.join(".local-tests");
    fs::create_dir_all(&parent).unwrap();
    let temporary = tempfile::Builder::new()
        .prefix("target-redis-layout-")
        .tempdir_in(parent)
        .unwrap();
    let directory = temporary.path();
    let config = directory.join("redis.conf");
    fs::write(&config, "bind 127.0.0.1\n").unwrap();
    let wsl = directory.join("wsl.exe");
    fs::write(&wsl, "fixed tool").unwrap();
    let configuration = common::binding(&config).unwrap();
    let redis = json!({"port":16390,"wsl":{"path":wsl,"sha256":common::binding(&wsl).unwrap()["sha256"]},
        "distribution":"fixture","pid":200,"started":"67890","executable":"/usr/bin/redis-server",
        "sha256":"9".repeat(64),"run_id":"3".repeat(40),"configuration":configuration});
    let review = json!({"services":{"redis":{"directory":directory}},"tools":{"redis_server":{
        "distribution":"fixture","resolved_path":"/usr/bin/redis-server","sha256":"9".repeat(64)}},
        "scopes":{"seed":{"redis":{"url":"redis://127.0.0.1:16390/0"}}}});
    let mut context = super::target_storage_model_tests::context(&backend, directory, directory);
    context.request["side"] = json!("seed");
    let mut fake = observer(&configuration, &redis_layout::wsl_path(directory).unwrap());
    redis_layout::verify(&context, &review, &redis, &fake).unwrap();
    assert_eq!(fake.calls.borrow().len(), 14);
    fake.wsl.insert(
        vec![
            "/usr/bin/readlink".into(),
            "-f".into(),
            redis_layout::wsl_path(&config).unwrap(),
        ],
        "/wrong/config".into(),
    );
    assert!(redis_layout::verify(&context, &review, &redis, &fake).is_err());
    let calls = fake.calls.borrow().len();
    fs::write(&config, "changed configuration").unwrap();
    assert!(redis_layout::verify(&context, &review, &redis, &fake).is_err());
    assert_eq!(fake.calls.borrow().len(), calls);
    fs::write(&config, "bind 127.0.0.1\n").unwrap();
    fs::write(&wsl, "changed tool").unwrap();
    assert!(redis_layout::verify(&context, &review, &redis, &fake).is_err());
    assert_eq!(fake.calls.borrow().len(), calls);
}
