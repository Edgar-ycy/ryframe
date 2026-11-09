//! 完整 prepare 的只读观察材料，供发布历史与 fresh status 共用。
use super::{Fixture, write};
use crate::recovery::clone::native::{digest, input};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn receipt(fixture: &Fixture, names: &mut BTreeSet<String>, stage: &str, value: &Value) {
    let name = format!("{stage}-{}.command.json", uuid::Uuid::new_v4().simple());
    write(&fixture.target.join(&name), value);
    assert!(names.insert(name));
}

pub(crate) fn command(command: Value, stdout: Value) -> Value {
    json!({"command":command,"returncode":0,"error_type":null,"stdout":stdout,"stderr":""})
}

pub(crate) fn prepared(fixture: &Fixture) -> BTreeSet<String> {
    let mut names = BTreeSet::from(["request.json".into(), "prepare.json".into()]);
    for db in fixture.request["target"]["databases"].as_array().unwrap() {
        receipt(
            fixture,
            &mut names,
            &format!("mysql-{}", db["key"].as_str().unwrap()),
            &command(
                json!([
                    fixture.request["tools"]["mysql"]["path"],
                    format!("--defaults-file={}", db["defaults_file"].as_str().unwrap()),
                    "--connect-timeout=5",
                    "--batch",
                    "--raw",
                    "--skip-column-names",
                    "--binary-mode",
                    "--default-character-set=utf8mb4"
                ]),
                json!(format!("{}\n", db["server_uuid"].as_str().unwrap())),
            ),
        );
    }
    for _ in 0..2 {
        kernel_observations(fixture, &mut names);
    }
    object_observations(fixture, &mut names);
    names
}

fn object_observations(fixture: &Fixture, names: &mut BTreeSet<String>) {
    for (stage, operation) in [
        ("bucket-readiness", "head-bucket"),
        ("bucket-versioning", "get-bucket-versioning"),
        ("scoped-list", "list-objects-v2"),
    ] {
        for bucket in input::BUCKETS {
            let list = operation == "list-objects-v2";
            let response = if list {
                json!({"IsTruncated":false,"Contents":[]})
            } else {
                json!({})
            };
            receipt(
                fixture,
                names,
                stage,
                &json!({
                    "transport":"rust-s3-sigv4","operation":operation,"bucket":bucket,
                    "returncode":0,"error_type":null,"response":response,"key":null,"destination":null,
                    "remote_writes":0,"scope_id":fixture.request["target"]["scope_id"],
                    "endpoint":fixture.request["target"]["s3"]["endpoint"],
                    "configuration_sha256":digest::plan_hash(&fixture.request["target"]["s3"]).unwrap(),
                    "prefix":if list {json!(format!("{}/",fixture.request["target"]["scope_id"].as_str().unwrap()))} else {Value::Null},
                    "continuation":null
                }),
            );
        }
    }
}

fn kernel_observations(fixture: &Fixture, names: &mut BTreeSet<String>) {
    let redis = &fixture.request["storage"]["redis"];
    let config_path = std::path::Path::new(redis["configuration"]["path"].as_str().unwrap());
    let config = if cfg!(windows) {
        crate::recovery::clone::native::target_verify::storage::redis_layout::wsl_path(config_path)
            .unwrap()
    } else {
        config_path.to_str().unwrap().to_owned()
    };
    let pid = redis["pid"].as_u64().unwrap();
    let mut fields = vec!["1"; 20];
    fields[19] = redis["started"].as_str().unwrap();
    let exe = redis["executable"].as_str().unwrap();
    for (args, stdout) in [
        (
            vec!["/usr/bin/cat".to_owned(), format!("/proc/{pid}/stat")],
            format!("{pid} (redis) {}\n", fields.join(" ")),
        ),
        (
            vec![
                "/usr/bin/readlink".into(),
                "-f".into(),
                format!("/proc/{pid}/exe"),
            ],
            format!("{exe}\n"),
        ),
        (
            vec!["/usr/bin/sha256sum".into(), exe.into()],
            format!("{}  {exe}\n", redis["sha256"].as_str().unwrap()),
        ),
        (
            vec!["/usr/bin/readlink".into(), "-f".into(), config.clone()],
            format!("{config}\n"),
        ),
        (
            vec!["/usr/bin/sha256sum".into(), config.clone()],
            format!(
                "{}  {config}\n",
                redis["configuration"]["sha256"].as_str().unwrap()
            ),
        ),
    ] {
        let mut argv = vec![
            redis["wsl"]["path"].clone(),
            json!("--distribution"),
            redis["distribution"].clone(),
            json!("--exec"),
        ];
        argv.extend(args.into_iter().map(Value::String));
        receipt(
            fixture,
            names,
            "redis-kernel",
            &command(json!(argv), json!(stdout)),
        );
    }
    let rustfs = &fixture.request["storage"]["rustfs"];
    let launch: Value = serde_json::from_slice(
        &std::fs::read(rustfs["launch_receipt"]["path"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    let review: Value = serde_json::from_slice(
        &std::fs::read(fixture.request["review"]["path"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    let name = format!("storage-layout-{}.json", uuid::Uuid::new_v4().simple());
    write(
        &fixture.target.join(&name),
        &json!({"process_receipt":rustfs["process_receipt"],"launch_receipt":rustfs["launch_receipt"],
        "actual_arguments_sha256":digest::plan_hash(&launch["arguments"]).unwrap(),"data_dir":review["services"]["rustfs"]["data_dir"],
        "environment_proof":"受控启动收据与凭据文件；不声称从内核枚举进程环境"}),
    );
    names.insert(name);
}
