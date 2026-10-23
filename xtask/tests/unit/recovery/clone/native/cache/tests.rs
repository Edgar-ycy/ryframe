use super::{
    super::{digest, json as document, resources::common},
    evidence, facts, model, process,
};
use serde_json::{Value, json};
use std::io::Cursor;

const BOOT: &str = "5a81dc04-9e9a-416b-8eab-1b059bddf489";

fn request() -> Value {
    json!({"scope_id":"cache-test","wsl":{"path":"D:\\wsl.exe","sha256":"a".repeat(64)},
        "distribution":"Ubuntu-24.04","launcher":"/usr/bin/redis-server","executable":"/usr/bin/redis-check-rdb",
        "sha256":"a".repeat(64),"configuration":{"path":"D:\\data\\redis.conf","bytes":5,"sha256":"b".repeat(64)},
        "directory":{"path":"D:\\data","device":1,"inode":2},
        "previous_identity":{"pid":200,"started":"50","executable":"/usr/bin/redis-check-rdb"},
        "previous_boot_id":BOOT,"previous_run_id":"a".repeat(40),"port":16390,"password":"bound-private-value",
        "timeout_seconds":1,"python":{"path":"/usr/bin/python3","executable":"/usr/bin/python3.12","sha256":"c".repeat(64)}})
}

#[test]
fn validates_exact_request_predecessor_and_direct_password() {
    let root = crate::workspace::root_dir();
    let request = request();
    // 文件路径关系按宿主平台核验；纯密码与前驱规则在所有平台运行。
    assert!(model::password(&request["password"]).is_ok());
    for invalid in [
        Value::Null,
        json!(false),
        json!(123),
        json!(""),
        json!(" \t"),
        json!("\0hidden-secret"),
    ] {
        let error = model::password(&invalid).unwrap_err();
        assert!(!error.to_string().contains("hidden-secret"));
    }
    let mut wrong = request.clone();
    wrong["port"] = json!(true);
    assert!(model::validate(&root, &wrong, false).is_err());
    wrong = request.clone();
    wrong["password_env"] = json!("APP_REDIS_PASSWORD");
    assert!(model::validate(&root, &wrong, false).is_err());
    #[cfg(windows)]
    {
        assert!(model::validate(&root, &request, false).is_ok());
        let mut initial = request.clone();
        for key in ["previous_identity", "previous_boot_id", "previous_run_id"] {
            initial[key] = Value::Null;
        }
        assert!(model::validate(&root, &initial, false).is_ok());
        for key in ["previous_boot_id", "previous_run_id"] {
            let mut invalid = initial.clone();
            invalid[key] = request[key].clone();
            assert!(model::validate(&root, &invalid, false).is_err());
        }
    }
}

#[test]
fn protocol_rejects_duplicates_oversize_trailing_and_unknown_fields() {
    let root = crate::workspace::root_dir();
    for raw in [
        b"".as_slice(),
        b"{\"operation\":1,\"operation\":2}",
        b"{} {}",
        b"{\"password\":\"secret\"}",
    ] {
        assert!(super::execute(&root, raw).is_err());
    }
    assert!(super::execute(&root, &vec![b' '; 1024 * 1024 + 1]).is_err());
    let value = json!({"format_version":1,"kind":"ryframe-cache-process","operation":"password","arguments":{"value":"private"}});
    assert_eq!(
        super::execute(&root, &serde_json::to_vec(&value).unwrap()).unwrap(),
        Value::Null
    );
}

#[test]
fn resp_rejects_truncation_non_utf8_server_errors_and_excess_without_secrets() {
    for raw in [
        b"-ERR secret password\r\n".as_slice(),
        b"$2\r\na\r\n",
        b"$1\r\n\xff\r\n",
        b"*33\r\n",
        b"+OK\n",
        b"$-1\r\n",
        b"*1\r\n*1\r\n*1\r\n+OK\r\n",
    ] {
        let error = facts::response(&mut Cursor::new(raw), 0).unwrap_err();
        assert!(!error.to_string().contains("secret password"));
    }
    assert_eq!(
        facts::response(
            &mut Cursor::new(b"*2\r\n$4\r\nbind\r\n$9\r\n127.0.0.1\r\n"),
            0
        )
        .unwrap(),
        json!(["bind", "127.0.0.1"])
    );
    assert!(facts::response(&mut Cursor::new(vec![b'a'; 1048577]), 0).is_err());
}

#[test]
fn readonly_redis_uses_bound_auth_info_and_exact_configuration_only() {
    let request = request();
    let mut commands = Vec::new();
    let result = facts::observe_with(&request, &mut |parts| {
        commands.push(parts.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>());
        Ok(match parts[0] {
            "AUTH" => json!("OK"),
            "INFO" => json!(format!(
                "process_id:300\r\nrun_id:{}\r\nconfig_file:/mnt/d/data/redis.conf\r\n",
                "b".repeat(40)
            )),
            "CONFIG" => json!([parts[2], model::configuration(&request).unwrap()[parts[2]]]),
            _ => panic!("禁止其他 Redis 命令"),
        })
    })
    .unwrap();
    assert_eq!(commands.len(), 10);
    assert_eq!(commands[0], vec!["AUTH", "bound-private-value"]);
    assert_eq!(commands[1], vec!["INFO", "server"]);
    assert!(
        commands[2..]
            .iter()
            .all(|command| command[..2] == ["CONFIG", "GET"])
    );
    assert!(!result.to_string().contains("bound-private-value"));
    let identity = json!({"pid":300});
    facts::check(&request, &identity, &result, &Value::Null).unwrap();
    for (key, value) in [
        ("process_id", json!("999")),
        ("run_id", request["previous_run_id"].clone()),
        ("config_file", json!("/another")),
        ("configuration_sha256", json!("0".repeat(64))),
    ] {
        let mut invalid = result.clone();
        invalid[key] = value;
        assert!(facts::check(&request, &identity, &invalid, &Value::Null).is_err());
    }
    assert!(facts::check(&request, &identity, &result, &json!("c".repeat(40))).is_err());
}

#[test]
fn info_duplicates_and_configuration_drift_fail_closed() {
    for duplicate in [true, false] {
        let request = request();
        let result = facts::observe_with(&request, &mut |parts| {
            Ok(match parts[0] {
                "AUTH" => json!("OK"),
                "INFO" if duplicate => json!("process_id:1\nprocess_id:2\n"),
                "INFO" => json!("process_id:1\n"),
                _ => json!([parts[2], "changed"]),
            })
        });
        assert!(result.is_err());
    }
}

#[test]
fn control_response_requires_exact_identity_boolean_fields_and_boot() {
    let identity =
        json!({"pid":300,"started":"60","executable":"/usr/bin/redis-check-rdb","boot_id":BOOT});
    let value = json!({"identity":identity,"alive":false,"boot_id":BOOT,"terminated":false});
    assert_eq!(
        process::validate_response(&serde_json::to_vec(&value).unwrap(), &identity).unwrap(),
        value
    );
    for (key, wrong) in [
        ("alive", json!(0)),
        ("terminated", json!("no")),
        ("boot_id", json!("unknown")),
        ("identity", json!({"pid":300})),
    ] {
        let mut invalid = value.clone();
        invalid[key] = wrong;
        assert!(
            process::validate_response(&serde_json::to_vec(&invalid).unwrap(), &identity).is_err()
        );
    }
    assert!(process::validate_response(&vec![b' '; 16385], &identity).is_err());
    assert!(process::validate_response(b"{} {}", &identity).is_err());
}

#[cfg(windows)]
struct Fixture {
    _directory: tempfile::TempDir,
    request: Value,
    output: std::path::PathBuf,
}

#[cfg(windows)]
impl Fixture {
    fn new() -> Self {
        let base = crate::workspace::root_dir().join(".local-tests");
        std::fs::create_dir_all(&base).unwrap();
        let directory = tempfile::tempdir_in(base).unwrap();
        let data = directory.path().join("data");
        std::fs::create_dir(&data).unwrap();
        let output = directory.path().join("process");
        std::fs::create_dir(&output).unwrap();
        let config = data.join("redis.conf");
        std::fs::write(&config, b"save \"\"\nappendonly no\n").unwrap();
        let wsl = directory.path().join("wsl.exe");
        std::fs::write(&wsl, b"stub").unwrap();
        let (device, inode) =
            common::device_inode_for(&data, &std::fs::metadata(&data).unwrap()).unwrap();
        let mut request = request();
        request["directory"] = json!({"path":data,"device":device,"inode":inode});
        request["configuration"] = common::binding(&config).unwrap();
        request["wsl"] = json!({"path":wsl,"sha256":digest::file(&wsl).unwrap().sha256});
        Self {
            _directory: directory,
            request,
            output,
        }
    }
    fn write(&self, name: &str, value: Value) {
        document::write_new(&self.output.join(name), &value).unwrap();
    }
    fn intent(&self) {
        let helpers = self.output.join("helpers");
        std::fs::create_dir(&helpers).unwrap();
        let mut sources = json!({});
        for (key, name) in [("cache", "cache.py"), ("process", "full_stack_process.py")] {
            let path = helpers.join(name);
            std::fs::write(&path, b"frozen helper").unwrap();
            let mut binding = common::binding(&path).unwrap();
            binding["path"] = json!(model::linux_path(path.to_str().unwrap()).unwrap());
            sources[key] = binding;
        }
        self.write("launch-request.json",json!({"format_version":1,"request":self.request,"request_sha256":digest::plan_hash(&self.request).unwrap(),"windows_output":self.output,"helpers":sources}));
        self.write("intent.json",json!({"request_sha256":digest::plan_hash(&self.request).unwrap(),"arguments":evidence::command(&self.request,&self.output,"serve").unwrap(),"launch":common::binding(&self.output.join("launch-request.json")).unwrap()}));
    }
    fn recorded(&self) -> Value {
        self.intent();
        self.write("launcher.json",json!({"identity":{"pid":400,"started":"70","executable":self.request["wsl"]["path"]},"intent":common::binding(&self.output.join("intent.json")).unwrap()}));
        self.write("linux-intent.json",json!({"request_sha256":digest::plan_hash(&self.request).unwrap(),"arguments":[self.request["launcher"],model::linux_path(self.request["configuration"]["path"].as_str().unwrap()).unwrap()],"supervisor":{}}));
        let mut binding = common::binding(&self.output.join("linux-intent.json")).unwrap();
        binding["path"] = json!(model::linux_path(binding["path"].as_str().unwrap()).unwrap());
        self.write("linux-process.json",json!({"identity":{"pid":300,"started":"60","executable":self.request["executable"],"boot_id":BOOT},"request_sha256":digest::plan_hash(&self.request).unwrap(),"intent":binding}));
        evidence::inspect(&self.request, &self.output).unwrap()
    }
}

#[cfg(windows)]
#[test]
fn frozen_receipts_bind_generation_tools_directory_and_fail_unknown_launches() {
    let fixture = Fixture::new();
    model::validate(&crate::workspace::root_dir(), &fixture.request, true).unwrap();
    assert!(
        evidence::inspect(&fixture.request, &fixture.output)
            .unwrap()
            .is_null()
    );
    let runtime = fixture.recorded();
    assert!(!runtime.to_string().contains("bound-private-value"));
    std::fs::write(
        fixture.request["wsl"]["path"].as_str().unwrap(),
        b"upgraded launcher",
    )
    .unwrap();
    assert_eq!(
        evidence::inspect(&fixture.request, &fixture.output).unwrap(),
        runtime
    );
    assert!(model::validate(&crate::workspace::root_dir(), &fixture.request, true).is_err());
    std::fs::write(fixture.output.join("helpers/cache.py"), b"changed").unwrap();
    assert!(evidence::inspect(&fixture.request, &fixture.output).is_err());
    let unknown = Fixture::new();
    unknown.intent();
    assert!(evidence::inspect(&unknown.request, &unknown.output).is_err());
}

#[cfg(windows)]
#[test]
fn control_rechecks_frozen_receipts_before_single_call_and_never_retries() {
    use std::os::windows::process::ExitStatusExt;
    let fixture = Fixture::new();
    let runtime = fixture.recorded();
    let mut calls = 0;
    let failure = process::call_with(&fixture.request, &runtime, "stop", |args, input| {
        calls += 1;
        assert!(args.iter().all(|arg| !arg.contains("bound-private-value")));
        assert_eq!(
            serde_json::from_slice::<Value>(input).unwrap(),
            runtime["linux_identity"]
        );
        Ok(std::process::Output {
            status: std::process::ExitStatus::from_raw(17),
            stdout: vec![],
            stderr: b"private password".to_vec(),
        })
    })
    .unwrap_err();
    assert_eq!(calls, 1);
    assert!(!failure.to_string().contains("private password"));
    let mut wrong = runtime.clone();
    wrong["linux_identity"]["started"] = json!("999");
    assert!(
        process::call_with(&fixture.request, &wrong, "stop", |_, _| panic!(
            "错误代次不得调用"
        ))
        .is_err()
    );
    assert!(
        process::call_with(&fixture.request, &runtime, "serve", |_, _| panic!(
            "不得隐式创建进程"
        ))
        .is_err()
    );
}

#[test]
fn linux_mapping_rejects_ambiguous_or_parent_paths() {
    assert_eq!(
        model::linux_path("D:\\路径 空间\\redis.conf").unwrap(),
        "/mnt/d/路径 空间/redis.conf"
    );
    for value in [
        "relative",
        "D:relative",
        "//server/share",
        "D:/../file",
        "D:/file:stream",
    ] {
        assert!(model::linux_path(value).is_err());
    }
}

#[test]
fn loopback_resp_authentication_uses_direct_request_password_and_redacts_errors() {
    use std::{
        io::{BufReader, Write},
        net::TcpListener,
        thread,
        time::Duration,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut request = request();
    request["port"] = json!(listener.local_addr().unwrap().port());
    let expected = model::configuration(&request).unwrap();
    let password = request["password"].clone();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        assert_eq!(
            facts::response(&mut reader, 0).unwrap(),
            json!(["AUTH", password])
        );
        stream.write_all(b"+OK\r\n").unwrap();
        assert_eq!(
            facts::response(&mut reader, 0).unwrap(),
            json!(["INFO", "server"])
        );
        let info = format!(
            "process_id:300\r\nrun_id:{}\r\nconfig_file:/mnt/d/data/redis.conf\r\n",
            "b".repeat(40)
        );
        stream
            .write_all(format!("${}\r\n{info}\r\n", info.len()).as_bytes())
            .unwrap();
        for (key, value) in expected.as_object().unwrap() {
            assert_eq!(
                facts::response(&mut reader, 0).unwrap(),
                json!(["CONFIG", "GET", key])
            );
            let value = value.as_str().unwrap();
            stream
                .write_all(
                    format!(
                        "*2\r\n${}\r\n{key}\r\n${}\r\n{value}\r\n",
                        key.len(),
                        value.len()
                    )
                    .as_bytes(),
                )
                .unwrap();
        }
    });
    let observed = facts::observe(&request).unwrap();
    server.join().unwrap();
    facts::check(&request, &json!({"pid":300}), &observed, &Value::Null).unwrap();
    assert!(!observed.to_string().contains("bound-private-value"));
}
