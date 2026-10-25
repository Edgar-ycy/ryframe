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
                json!(["fixture-mysql"]),
                json!(format!("{}\n", db["server_uuid"].as_str().unwrap())),
            ),
        );
    }
    let pid = fixture.request["storage"]["redis"]["pid"].as_u64().unwrap();
    for _ in 0..2 {
        for index in 0..5 {
            let (args, stdout) = if index == 0 {
                (
                    json!(["/usr/bin/cat", format!("/proc/{pid}/stat")]),
                    json!(format!("{pid} (redis) {}", vec!["1"; 20].join(" "))),
                )
            } else {
                (
                    json!(["fixture-redis", format!("observation-{index}")]),
                    json!("fixed\n"),
                )
            };
            receipt(fixture, &mut names, "redis-kernel", &command(args, stdout));
        }
        let name = format!("storage-layout-{}.json", uuid::Uuid::new_v4().simple());
        write(&fixture.target.join(&name), &fixture.request["storage"]);
        names.insert(name);
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
