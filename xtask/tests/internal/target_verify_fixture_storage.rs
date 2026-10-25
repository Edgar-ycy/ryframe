use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_json::{Value, json};

use super::{Fixture, SCOPE, bound, tool, write};
use crate::{
    Result,
    recovery::clone::native::{
        digest, json as json_io,
        target_verify::storage::{StorageObserver, redis_layout, rustfs},
    },
};

pub(super) fn files(fixture: &mut Fixture) -> Value {
    let storage = fixture.local.join("storage");
    fs::create_dir(&storage).unwrap();
    let data = storage.join("objects");
    fs::create_dir(&data).unwrap();
    let expected = json!({"scope_id":SCOPE,"api":"http://127.0.0.1:29000","console":"http://127.0.0.1:29001","data_dir":data,"process_receipt":storage.join("process.json")});
    let identity = json!({"pid":42,"started":"12345","executable":fixture.local.join("rustfs")});
    let mut credentials = json!({});
    for (field, value) in [
        ("access_key", "fixture-access"),
        ("secret_key", "fixture-secret"),
    ] {
        let path = storage.join(format!("{field}.txt"));
        fs::write(&path, format!("{value}\n")).unwrap();
        credentials[field] = bound(&path);
    }
    write(
        &storage.join("launch.json"),
        &json!({"format_version":1,"scope_id":SCOPE,"identity":identity,"arguments":rustfs::arguments(&expected,&identity,&data,&credentials).unwrap(),
        "credential_files":credentials}),
    );
    write(
        &storage.join("process.json"),
        &json!({"format_version":1,"role":"rustfs","scope_id":SCOPE,"lifecycle":"running","identity":identity,
        "api_url":expected["api"],"console_url":expected["console"],"data_dir":data}),
    );
    let redis = fixture.local.join("redis");
    fs::create_dir(&redis).unwrap();
    let linux = redis_layout::wsl_path(&redis).unwrap();
    fs::write(redis.join("redis.conf"),format!("bind 127.0.0.1\nport 26379\ndir \"{linux}\"\ndatabases 1\nprotected-mode yes\nsave \"\"\nappendonly no\ndaemonize no\n")).unwrap();
    fixture.request["storage"] = json!({"rustfs":{"identity":identity,"sha256":tool(&fixture.local.join("rustfs"))["sha256"],
        "process_receipt":bound(&storage.join("process.json")),"launch_receipt":bound(&storage.join("launch.json"))},
        "redis":{"port":26379,"wsl":tool(&fixture.local.join("wsl")),"distribution":"fixture","pid":200,"started":"67890","executable":"/usr/bin/redis-server",
            "sha256":"9".repeat(64),"run_id":"3".repeat(40),"configuration":bound(&redis.join("redis.conf"))}});
    json!({"rustfs":expected,"redis":{"directory":redis}})
}

pub(in super::super) struct ExternalFacts {
    identity: Value,
    arguments: Vec<String>,
    endpoints: BTreeSet<String>,
    configuration: BTreeMap<String, String>,
    config_path: String,
    config_sha: String,
    identities: Cell<usize>,
    arg_reads: Cell<usize>,
    listeners: RefCell<BTreeSet<String>>,
    config_reads: RefCell<BTreeSet<String>>,
    kernel_reads: Cell<usize>,
}

impl ExternalFacts {
    pub(super) fn new(fixture: &Fixture) -> Self {
        let path = Path::new(
            fixture.request["storage"]["rustfs"]["launch_receipt"]["path"]
                .as_str()
                .unwrap(),
        );
        let launch = json_io::read(path).unwrap();
        let config = Path::new(
            fixture.request["storage"]["redis"]["configuration"]["path"]
                .as_str()
                .unwrap(),
        );
        let configuration = fs::read_to_string(config)
            .unwrap()
            .lines()
            .map(|line| {
                let (key, value) = line.split_once(' ').unwrap();
                (key.to_owned(), value.trim_matches('"').to_owned())
            })
            .collect();
        Self {
            identity: launch["identity"].clone(),
            arguments: serde_json::from_value(launch["arguments"].clone()).unwrap(),
            endpoints: BTreeSet::from([
                "http://127.0.0.1:29000".into(),
                "http://127.0.0.1:29001".into(),
            ]),
            configuration,
            config_path: redis_layout::wsl_path(config).unwrap(),
            config_sha: digest::file(config).unwrap().sha256,
            identities: Cell::new(0),
            arg_reads: Cell::new(0),
            listeners: RefCell::new(BTreeSet::new()),
            config_reads: RefCell::new(BTreeSet::new()),
            kernel_reads: Cell::new(0),
        }
    }

    pub(in super::super) fn assert_complete(&self) {
        assert!(self.identities.get() >= 5);
        assert_eq!(self.arg_reads.get(), 1);
        assert_eq!(*self.listeners.borrow(), self.endpoints);
        assert_eq!(self.config_reads.borrow().len(), 8);
        assert_eq!(self.kernel_reads.get(), 5);
    }
}

impl StorageObserver for ExternalFacts {
    fn process_identity(&self, pid: u32) -> Result<Option<Value>> {
        self.identities.set(self.identities.get() + 1);
        if self.identity["pid"].as_u64() == Some(u64::from(pid)) {
            Ok(Some(self.identity.clone()))
        } else {
            Err("fixture 未声明的进程观察".into())
        }
    }

    fn arguments(
        &self,
        _context: &crate::recovery::clone::native::target_verify::storage::StorageInputs<'_>,
        identity: &Value,
    ) -> Result<Vec<String>> {
        assert_eq!(identity, &self.identity);
        self.arg_reads.set(self.arg_reads.get() + 1);
        Ok(self.arguments.clone())
    }

    fn listener(
        &self,
        _context: &crate::recovery::clone::native::target_verify::storage::StorageInputs<'_>,
        identity: &Value,
        url: &str,
    ) -> Result<()> {
        assert_eq!(identity, &self.identity);
        assert!(self.endpoints.contains(url));
        self.listeners.borrow_mut().insert(url.into());
        Ok(())
    }

    fn wsl(
        &self,
        _context: &crate::recovery::clone::native::target_verify::storage::StorageInputs<'_>,
        redis: &Value,
        args: &[String],
    ) -> Result<String> {
        self.kernel_reads.set(self.kernel_reads.get() + 1);
        let path = args.last().ok_or("fixture WSL path")?;
        match args.first().map(String::as_str) {
            Some("/usr/bin/cat") if path == "/proc/200/stat" => {
                let mut fields = vec!["0"; 20];
                fields[0] = "S";
                fields[19] = "67890";
                Ok(format!("200 (redis fixture) {}", fields.join(" ")))
            }
            Some("/usr/bin/readlink") if path == "/proc/200/exe" => {
                Ok("/usr/bin/redis-server".into())
            }
            Some("/usr/bin/readlink") if path == &self.config_path => Ok(self.config_path.clone()),
            Some("/usr/bin/sha256sum") if path == "/usr/bin/redis-server" => {
                Ok(format!("{}  {path}", redis["sha256"].as_str().unwrap()))
            }
            Some("/usr/bin/sha256sum") if path == &self.config_path => {
                Ok(format!("{}  {path}", self.config_sha))
            }
            _ => Err("fixture 未声明的 WSL 观察".into()),
        }
    }

    fn redis(
        &self,
        context: &crate::recovery::clone::native::target_verify::storage::StorageInputs<'_>,
        _redis: &Value,
        parts: &[&str],
    ) -> Result<Value> {
        match parts {
            ["INFO", "server"] => Ok(json!(format!(
                "process_id:200\r\nrun_id:{}\r\nconfig_file:{}\r\n",
                "3".repeat(40),
                self.config_path
            ))),
            ["SCAN", "0", "MATCH", pattern, "COUNT", "1000"] => {
                assert_eq!(*pattern, format!("ryframe:{{{SCOPE}}}:*"));
                Ok(json!([
                    "0",
                    [format!("ryframe:{{{SCOPE}}}:.ryframe-owner")]
                ]))
            }
            ["GET", key] if *key == format!("ryframe:{{{SCOPE}}}:.ryframe-owner") => {
                Ok(json!(format!("ryframe-owner:v1:{SCOPE}:redis")))
            }
            ["GET", key] if Some(*key) == context.request["reset"]["sentinel_key"].as_str() => {
                Ok(context.request["reset"]["sentinel_value"].clone())
            }
            ["CONFIG", "GET", key] => {
                let value = self
                    .configuration
                    .get(*key)
                    .ok_or("fixture 未声明 Redis 配置")?;
                self.config_reads.borrow_mut().insert((*key).into());
                Ok(json!([key, value]))
            }
            _ => Err("fixture 未声明的 Redis 只读查询".into()),
        }
    }
}

pub(super) fn initialized() -> Value {
    json!({"keys":[format!("ryframe:{{{SCOPE}}}:.ryframe-owner")],"owner":format!("ryframe-owner:v1:{SCOPE}:redis"),"sentinel":"devex-fresh:target-verify-fixture"})
}
