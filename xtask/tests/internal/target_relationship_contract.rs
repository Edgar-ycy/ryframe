use super::{Fixture, bound, digest, request, review_binding, write};
use crate::{
    cli::{FixtureControlCommand, FixtureSuccessorCommand, FixtureSuccessorRelationship},
    recovery::{
        clone::native::{
            cache::binding::source::relationship::require_independent_b1_disjoint, json as document,
        },
        fixture_control,
    },
};
use serde_json::{Value, json};
use std::{fs, path::Path};

const SIDES: [&str; 3] = ["seed", "base", "candidate"];

fn review(fixture: &Fixture, label: &str, port: u16) -> Value {
    let mut value = fixture.review.clone();
    let root = fixture.local.join("reference-fixture").join(label);
    value["future_root"] = json!(root);
    value["tools"]["wsl"] = value["tools"]["mysql"].clone();
    value["tools"]["redis_python"] = value["tools"]["redis_server"].clone();
    value["services"]["rustfs"] = json!({
        "api":format!("http://127.0.0.1:{port}"),
        "console":format!("http://127.0.0.1:{}", port + 1),
        "data_dir":root.join("objects"),
    });
    value["services"]["redis"] = json!({
        "directory":root.join("redis"), "endpoint":format!("127.0.0.1:{}", port + 2),
    });
    for (side, index) in SIDES.into_iter().zip(0_u16..) {
        let selected = &mut value["scopes"][side];
        let scope = format!("{label}-{side}");
        let namespace = format!("ryframe:{{{scope}}}:");
        selected["backend_dir"] = json!(fixture.backend);
        selected["scope_id"] = json!(scope);
        selected["runtime_dir"] = json!(root.join(format!("{side}-runtime")));
        selected["identity_ledger"] = json!(root.join(format!("{side}-identities")));
        selected["redis"]["namespace"] = json!(namespace);
        selected["redis"]["ownership_key"] = json!(format!("{namespace}.ryframe-owner"));
        selected["redis"]["ownership_value"] = json!(format!("ryframe-owner:v1:{scope}:redis"));
        let first = port + 3 + index * 3;
        selected["api_url"] = json!(format!("http://127.0.0.1:{first}"));
        selected["worker_ready_url"] = json!(format!("http://127.0.0.1:{}/readyz", first + 1));
        selected["frontend_url"] = json!(format!("http://127.0.0.1:{}", first + 2));
        for database in selected["databases"].as_array_mut().unwrap() {
            database["database"] = json!(format!(
                "{label}_{}",
                database["database"].as_str().unwrap()
            ));
        }
    }
    value
}

fn sign_review(fixture: &Fixture, label: &str, mut value: Value, path: &Path) -> Value {
    let pending = fixture.local.join(format!("{label}-pending.json"));
    value["ready_for_execution"] = json!(false);
    write(&pending, &value);
    value["ready_for_execution"] = json!(true);
    value["services"]["rustfs"]["scope_id"] = json!(format!(
        "services-{}",
        value["scopes"]["seed"]["scope_id"].as_str().unwrap()
    ));
    value["services"]["rustfs"]["process_receipt"] = json!(
        fixture
            .local
            .join("reference-fixture")
            .join(label)
            .join("service-run/rustfs/process.json")
    );
    value["preflight"] = json!({
        "format_version":1, "kind":"reference-fixture-tool-preflight", "status":"verified",
        "supersedes":bound(&pending), "tools":value["tools"],
    });
    write(path, &value);
    value
}

fn write_request(fixture: &Fixture, ready: &Value, review_path: &Path, side: &str, path: &Path) {
    let mut value = fixture.request.clone();
    value["id"] = json!(path.file_stem().unwrap().to_str().unwrap());
    value["review"] = review_binding(review_path, ready);
    value["side"] = json!(side);
    value["target"]["scope_id"] = ready["scopes"][side]["scope_id"].clone();
    for (item, (key, _, _)) in value["target"]["databases"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(request::KEYS)
    {
        let planned = ready["scopes"][side]["databases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|database| database["key"] == key)
            .unwrap();
        item["database"] = planned["database"].clone();
    }
    write(path, &value);
}

fn publish(fixture: &Fixture, options: &FixtureSuccessorRelationship) -> crate::Result<()> {
    fixture_control::run(
        &FixtureControlCommand::Successor(FixtureSuccessorCommand::Relationship(options.clone())),
        &fixture.backend,
    )
}

fn read(fixture: &Fixture, descriptor: &Value) -> crate::Result<Value> {
    super::super::super::binding::execute(&serde_json::to_vec(&json!({
        "format_version":1, "operation":"review-successor", "backend":fixture.backend,
        "arguments":{"descriptor":descriptor},
    }))?)
}

#[test]
fn successor_relationship_publishes_once_and_revalidates_all_three_sides() {
    let fixture = Fixture::new();
    let evidence = fixture.local.join("successor-relationship");
    fs::create_dir(&evidence).unwrap();
    let path = |name| evidence.join(name);
    let registration = path("registration.json");
    write(
        &registration,
        &json!({"kind":"devex-clone-seed-source-registration"}),
    );
    let options = FixtureSuccessorRelationship {
        source_result: path("source-result.json"),
        predecessor_review: path("historical-review.json"),
        predecessor_request: path("historical-seed.json"),
        successor_review: path("next-review.json"),
        seed_request: path("next-seed.json"),
        base_request: path("next-base.json"),
        candidate_request: path("next-candidate.json"),
        id: "seed-successor".into(),
        output: path("relationship.json"),
    };
    write(
        &options.source_result,
        &json!({
            "status":"seed_source_registered", "registration":bound(&registration),
            "remote_writes":0, "outbox_drained":true, "restore_qualified":false,
        }),
    );
    let old = review(&fixture, "historical", 21000);
    let old_ready = sign_review(
        &fixture,
        "historical",
        old.clone(),
        &options.predecessor_review,
    );
    write_request(
        &fixture,
        &old_ready,
        &options.predecessor_review,
        "seed",
        &options.predecessor_request,
    );
    let next = sign_review(
        &fixture,
        "next",
        review(&fixture, "next", 22000),
        &options.successor_review,
    );
    let requests = [
        &options.seed_request,
        &options.base_request,
        &options.candidate_request,
    ];
    for (side, request_path) in SIDES.into_iter().zip(requests) {
        write_request(
            &fixture,
            &next,
            &options.successor_review,
            side,
            request_path,
        );
    }
    publish(&fixture, &options).unwrap();
    let original = fs::read(&options.output).unwrap();
    let descriptor = bound(&options.output);
    let value = read(&fixture, &descriptor).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&original).unwrap(), value);
    let mut unhashed = value.clone();
    let hash = unhashed
        .as_object_mut()
        .unwrap()
        .remove("relationship_sha256")
        .unwrap();
    assert_eq!(hash, digest::plan_hash(&unhashed).unwrap());
    for side in ["predecessor", "successor"] {
        let projection = &value["collision_projection"][side];
        assert_eq!(projection["databases"].as_array().unwrap().len(), 12);
        assert_eq!(projection["objects"].as_array().unwrap().len(), 15);
    }
    assert!(publish(&fixture, &options).is_err());
    assert_eq!(fs::read(&options.output).unwrap(), original);
    check_independent_measurement(&fixture, &descriptor, &old, &next);
    assert_eq!(fs::read(&options.output).unwrap(), original);

    let failed = FixtureSuccessorRelationship {
        output: path("rejected.json"),
        ..options.clone()
    };
    reject_modified_candidate_request(&fixture, &failed, &descriptor, &next);
    reject_collisions(&fixture, &failed, &old, &next);
}

fn measurement_review(mut value: Value, root: &Path) -> Value {
    value["kind"] = json!("review-only-b1-runtime-measurement-with-readonly-preflight");
    value["future_root"] = json!(root);
    value["services"]["rustfs"]["data_dir"] = json!(root.join("rustfs"));
    value["services"]["redis"]["directory"] = json!(root.join("redis"));
    let mut candidate = value["scopes"]["candidate"].take();
    candidate["runtime_dir"] = json!(root.join("runtime-candidate"));
    candidate["identity_ledger"] = json!(root.join("identities-candidate"));
    candidate["objects"]["endpoint"] = value["services"]["rustfs"]["api"].clone();
    let redis = value["services"]["redis"]["endpoint"].as_str().unwrap();
    candidate["redis"]["url"] = json!(format!("redis://{redis}/0"));
    value["scopes"] = json!({"candidate":candidate});
    let tools = value["tools"].as_object_mut().unwrap();
    tools.remove("wsl");
    tools.remove("redis_python");
    value["tools"]["redis_server"] = json!({
        "platform":"windows","path":"D:/tools/redis-server.exe","sha256":"b".repeat(64),
        "version":"Redis server v=7.4.11 bits=64","dlls":{"msys-2.0.dll":"a".repeat(64)},
    });
    value
}

fn check_independent_measurement(
    fixture: &Fixture,
    descriptor: &Value,
    previous: &Value,
    successor: &Value,
) {
    let compare =
        |review: &Value| require_independent_b1_disjoint(&fixture.backend, descriptor, review);
    let template = review(fixture, "measurement", 23000);
    let root = Path::new(template["future_root"].as_str().unwrap());
    let candidate = measurement_review(template.clone(), root);
    let projection = compare(&candidate).unwrap();
    assert_eq!(
        projection["scopes"],
        json!([{"owner":"measurement.candidate","identity":"measurement-candidate"}])
    );
    for (field, count) in [
        ("databases", 4),
        ("redis", 1),
        ("objects", 5),
        ("paths", 5),
        ("sockets", 6),
    ] {
        assert_eq!(projection[field].as_array().unwrap().len(), count);
    }

    let mut duplicate = candidate;
    let previous_database = &previous["scopes"]["base"]["databases"][0];
    let database = &mut duplicate["scopes"]["candidate"]["databases"][0];
    assert_eq!(
        database["expected_server_uuid"],
        previous_database["expected_server_uuid"]
    );
    database["database"] = json!(
        previous_database["database"]
            .as_str()
            .unwrap()
            .to_ascii_uppercase()
    );
    assert_eq!(
        compare(&duplicate).unwrap_err().to_string(),
        "successor 资源与历史计划发生碰撞"
    );

    let nested = Path::new(successor["future_root"].as_str().unwrap()).join("measurement");
    assert_eq!(
        compare(&measurement_review(template, &nested))
            .unwrap_err()
            .to_string(),
        "successor 路径与历史计划重叠"
    );
}

fn reject_modified_candidate_request(
    fixture: &Fixture,
    options: &FixtureSuccessorRelationship,
    descriptor: &Value,
    next: &Value,
) {
    let mut changed = document::read(&options.candidate_request).unwrap();
    changed["storage"]["redis"]["port"] = json!(26380);
    write(&options.candidate_request, &changed);
    assert!(read(fixture, descriptor).is_err());
    let mismatched = "successor 三侧请求必须共享服务代次且使用不同 ID";
    assert_eq!(
        publish(fixture, options).unwrap_err().to_string(),
        mismatched
    );
    changed["storage"]["redis"]["port"] = json!(26379);
    changed["id"] = json!("next-base");
    write(&options.candidate_request, &changed);
    assert_eq!(
        publish(fixture, options).unwrap_err().to_string(),
        mismatched
    );
    assert!(!options.output.exists());
    write_request(
        fixture,
        next,
        &options.successor_review,
        "candidate",
        &options.candidate_request,
    );
}

fn reject_collisions(
    fixture: &Fixture,
    options: &FixtureSuccessorRelationship,
    old: &Value,
    next: &Value,
) {
    let bind_old = |value| {
        let ready = sign_review(fixture, "historical", value, &options.predecessor_review);
        write_request(
            fixture,
            &ready,
            &options.predecessor_review,
            "seed",
            &options.predecessor_request,
        );
    };
    for (changed_field, next_field, message) in [
        (
            "/scopes/candidate/databases/0/database",
            "/scopes/base/databases/0/database",
            "successor 资源与历史计划发生碰撞",
        ),
        (
            "/scopes/candidate/runtime_dir",
            "/scopes/base/runtime_dir",
            "successor 路径与历史计划重叠",
        ),
        (
            "/scopes/candidate/frontend_url",
            "/scopes/base/frontend_url",
            "successor 资源与历史计划发生碰撞",
        ),
        (
            "/scopes/candidate/redis",
            "/scopes/base/redis",
            "successor 资源与历史计划发生碰撞",
        ),
    ] {
        let mut changed = old.clone();
        *changed.pointer_mut(changed_field).unwrap() = next.pointer(next_field).unwrap().clone();
        if changed_field.ends_with("/redis") {
            changed["scopes"]["candidate"]["scope_id"] = next["scopes"]["base"]["scope_id"].clone();
        }
        bind_old(changed);
        assert_eq!(
            publish(fixture, options).unwrap_err().to_string(),
            message,
            "{changed_field}"
        );
        assert!(!options.output.exists());
    }
    let mut changed = old.clone();
    changed["scopes"]["candidate"]["api_url"] = json!("https://127.1:22040");
    bind_old(changed.clone());
    assert_eq!(
        publish(fixture, options).unwrap_err().to_string(),
        "successor HTTP 端点必须是明确的本机 loopback"
    );
    assert!(!options.output.exists());
    changed["scopes"]["candidate"]["api_url"] = json!("https://[::1]/readyz");
    bind_old(changed);
    publish(fixture, options).unwrap();
    let verified = read(fixture, &bound(&options.output)).unwrap();
    assert!(
        verified["collision_projection"]["predecessor"]["sockets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["identity"] == "::1:443")
    );
}
