use super::*;
use serde_json::Map;
use std::fs;

fn write(root: &Path, name: &str, value: &Value) -> Value {
    let file = root.join(".local-tests").join(name);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    documents::write_new(&file, value).unwrap();
    common::binding(&file).unwrap()
}
fn fields(names: &[&str], value: &Value) -> Value {
    Value::Object(
        names
            .iter()
            .map(|name| ((*name).into(), value.clone()))
            .collect::<Map<_, _>>(),
    )
}

fn image_parts(copy_plan: &Value, plan: &Value, objects: &[Value]) -> (Value, Value) {
    let mut databases = json!({});
    for item in copy_plan["databases"].as_array().unwrap() {
        let key = item["key"].as_str().unwrap();
        let mut placements = Vec::new();
        if key == "shared-control" {
            placements.push(json!({"tenant_id":"system"}));
        }
        for (index, target) in plan["dataset"]["tenant_targets"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            if target == key {
                placements.push(json!({"tenant_id":format!("restore-source-unit-{:02}",index+1)}));
            }
        }
        databases[key] = json!({"target":{"database":{"key":key,"placements":placements,"tables":[{"table":"sys_post","rows":item["tables"]["sys_post"],"sha256":"a".repeat(64)}]}}});
    }
    let mut inventory = json!({});
    for bucket in crate::recovery::clone::native::input::BUCKETS {
        inventory[bucket] = json!({});
    }
    for item in objects {
        inventory[item["bucket"].as_str().unwrap()][item["target_key"].as_str().unwrap()] = json!({"bytes":item["artifact"]["bytes"],"sha256":item["artifact"]["sha256"],"metadata":item["metadata"],"identity":{"ETag":"bound"}});
    }
    (databases, inventory)
}

struct Origin {
    plan: Value,
    dataset: Value,
    objects: Vec<Value>,
    plan_descriptor: Value,
    dataset_descriptor: Value,
}
fn origin(root: &Path) -> Origin {
    fs::create_dir_all(root.join(".local-tests/config")).unwrap();
    let config = root.join(".local-tests/config");
    fs::write(config.join("app.test.toml"), "scope_id='source-unit'\n").unwrap();
    let hash = digest::file(&config.join("app.test.toml")).unwrap();
    let mut binding = json!({"app_env":"test","directory":config,"files":{"app.test.toml":{"bytes":hash.bytes,"sha256":hash.sha256}}});
    binding["sha256"] = json!(digest::plan_hash(&binding).unwrap());
    let tool = root.join(".local-tests/tool");
    fs::write(&tool, "explicit-fixture-tool").unwrap();
    let tool_hash = digest::file(&tool).unwrap().sha256;
    let defaults = root.join(".local-tests/client.cnf");
    fs::write(
        &defaults,
        "[client]\nhost=127.0.0.1\nport=3306\nuser=user\npassword=private-db\nssl-mode=REQUIRED\n",
    )
    .unwrap();
    let targets = vec![
        "shared",
        "shared",
        "shared",
        "shared",
        "shared-control",
        "shared-control",
        "shared-control",
        "shared-control",
        "dedicated-a",
        "dedicated-b",
    ];
    let mut plan = json!({"format_version":1,"id":"lineage-unit","target_side":"base","work_dir":root.join(".local-tests/origin"),
            "dataset":{"tenant_targets":targets,"timeout_seconds":21600},
            "tools":{"mysql":{"path":tool,"sha256":tool_hash},"mysqldump":{"path":tool,"sha256":tool_hash}}});
    for side in ["source", "target"] {
        let databases=["shared-control","shared","dedicated-a","dedicated-b"].iter().map(|key|json!({"key":key,
                "database":format!("{side}_{}",key.replace('-',"_")),"server_uuid":format!("server-{side}"),
                "defaults_file":defaults,"defaults_sha256":digest::file(&defaults).unwrap().sha256,
                "kind":if *key=="shared-control" {"combined"} else {"tenant"},"mode":if key.starts_with("dedicated") {"dedicated"} else {"shared"}})).collect::<Vec<_>>();
        plan[side] = json!({"scope_id":if side=="source" {"restore-source-unit"} else {"restore-target-unit"},
                "api_url":"http://127.0.0.1:18210","frontend_url":"http://127.0.0.1:4190","worker_ready_url":"http://127.0.0.1:18211/readyz",
                "runtime_dir":root.join(".local-tests").join(format!("{side}-runtime")),"databases":databases,
                "s3":{"endpoint":"http://127.0.0.1:29000","region":"us-east-1","configuration":binding}});
    }
    let plan_descriptor = write(root, "origin/plan.json", &plan);
    let mut tenants=(0..11).map(|index|json!({"tenant_id":if index==0 {"system".into()} else {format!("restore-source-unit-{index:02}")},
            "username":format!("user-{index}"),"records":if index<10 {9091} else {9090},
            "posts":(0..3).map(|p|json!({"id":(1000+index*3+p).to_string(),"code":format!("P{p}"),"name":"岗位"})).collect::<Vec<_>>(),"files":[]})).collect::<Vec<_>>();
    let mut objects = Vec::new();
    for index in 0..256 {
        let id = tenants[index % 11]["tenant_id"].as_str().unwrap();
        let file_path = format!("{id}/object-{index:03}.bin");
        let item = json!({"file_id":(2000+index).to_string(),"file_name":format!("object-{index:03}.bin"),"file_path":file_path,
                "file_url":format!("/{file_path}"),"bytes":4*1024*1024,"sha256":digest::plan_hash(&json!(file_path)).unwrap()});
        objects.push(json!({"bucket":"uploads","source_key":format!("restore-source-unit/{file_path}"),
                "target_key":format!("perf-seed-unit/{file_path}"),"artifact":{"bytes":item["bytes"],"sha256":item["sha256"]},"metadata":{"tenant-id":id}}));
        tenants[index % 11]["files"]
            .as_array_mut()
            .unwrap()
            .push(item);
    }
    objects.push(json!({"bucket":"uploads","source_key":"restore-source-unit/system/probe.txt","target_key":"perf-seed-unit/system/probe.txt",
            "artifact":{"bytes":7,"sha256":"f".repeat(64)},"metadata":{}}));
    let dataset = json!({"format_version":1,"plan_sha256":digest::plan_hash(&plan).unwrap(),"source_scope_id":"restore-source-unit",
            "started_at":"2026-09-01T00:00:00Z","records":100000,"object_bytes":1024*1024*1024,"tenants":tenants,"request_interval_ms":1000,"post_concurrency":4,"completed_at":"2026-09-03T00:00:00Z"});
    let dataset_descriptor = write(root, "origin/result.json", &dataset);
    Origin {
        plan,
        dataset,
        objects,
        plan_descriptor,
        dataset_descriptor,
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    args: Value,
    dataset: Value,
    plan: Value,
}
impl Fixture {
    fn new() -> Self {
        let base = crate::workspace::root_dir().join(".local-tests/source-lineage-unit");
        fs::create_dir_all(&base).unwrap();
        let temp = tempfile::tempdir_in(base).unwrap();
        let root = temp.path().to_path_buf();
        let Origin {
            plan,
            dataset,
            objects,
            plan_descriptor,
            dataset_descriptor,
        } = origin(&root);
        let copy_plan = json!({"plan_sha256":"1".repeat(64),"copy_stage":"source_to_seed","source_scope":"restore-source-unit","target_scope":"perf-seed-unit","objects":objects,
            "databases":[{"key":"shared-control","tables":{"sys_post":100044}},{"key":"shared","tables":{"sys_post":0}},{"key":"dedicated-a","tables":{"sys_post":0}},{"key":"dedicated-b","tables":{"sys_post":0}}]});
        write(
            &root,
            "copy-run/copy/plan.json",
            &json!({"format_version":1,"input_path":root.join(".local-tests/copy-run/copy/input.json"),"input_file":{"bytes":1,"sha256":"a".repeat(64)},"plan":copy_plan}),
        );
        let copy_result = write(
            &root,
            "copy-run/copy/result.json",
            &json!({"status":"data_steps_verified","plan_sha256":"1".repeat(64),"fresh_target_sha256":"2".repeat(64),"generation_sha256":"3".repeat(64),"source_export_sha256":"4".repeat(64)}),
        );
        let stage = write(
            &root,
            "copy-run/copy/stage.json",
            &json!({"status":"passed"}),
        );
        let head = write(&root, "copy-run/copy/head.json", &json!({"sequence":52}));
        let post = json!({"reference_plan":plan_descriptor,"dataset":dataset_descriptor,"copy_result":copy_result,"copy_stage_receipt":stage,"ledger_head":head});
        let post_descriptor = write(&root, "copy-run/post-copy.json", &post);
        let copy = json!({"fresh_target_sha256":"2".repeat(64),"generation_sha256":"3".repeat(64),"source_export_sha256":"4".repeat(64),
            "plan_sha256":"1".repeat(64),"ledger_head_sha256":head["sha256"],"stage_receipt_sha256":stage["sha256"]});
        let target = json!({"format_version":1,"kind":"devex-copy-business-target","source_plan_sha256":dataset["plan_sha256"],"source_scope_id":"restore-source-unit",
            "target":{"scope_id":"perf-seed-unit","api_url":"http://127.0.0.1:18210","frontend_url":"http://127.0.0.1:4190"},"copy":copy});
        let target_descriptor = write(
            &root,
            "copy-run/post-copy/attempt-0029/business-target.json",
            &target,
        );
        let evidence = write(
            &root,
            "copy-run/post-copy/attempt-0029/business-result.json",
            &json!({"format_version":1,"status":"copy_existing_data_verified","side":"copy_target",
            "scope_id":"perf-seed-unit","source_scope_id":"restore-source-unit","plan_sha256":dataset["plan_sha256"],"target_binding_sha256":digest::plan_hash(&target).unwrap(),
            "copy":copy,"clone_verified":false,"restore_success":false,"tenants":11,"posts":33,"files":256,
            "actions":{"business":"read_only","objects":"read_only","session":"login_logout"},"input_files":{"plan_sha256":plan_descriptor["sha256"],"dataset_sha256":dataset_descriptor["sha256"],"target_binding_sha256":target_descriptor["sha256"]}}),
        );
        let c29 = write(
            &root,
            "copy-run/results/0029.json",
            &json!({"status":"post_copy_existing_data_verified","evidence":evidence,"objects":{"business_objects":256,"additional_objects":1,"additional_objects_downloaded":true,"all_scoped_keys_verified":true},"registration":post_descriptor,"restore_qualified":false,"worker_must_remain_stopped":true}),
        );
        let source_request =
            json!({"source":{"scope_id":"perf-seed-unit","api_url":"http://127.0.0.1:18210"}});
        let source_descriptor = write(&root, "copy-run/source-request.json", &source_request);
        let manifest = json!({"copy_stage":"source_to_seed"});
        let manifest_descriptor = write(&root, "copy-run/manifest.json", &manifest);
        let mut registration = fields(fields::SOURCE_REGISTRATION_FIELDS, &plan_descriptor);
        for (key, value) in [
            ("format_version", json!(1)),
            ("kind", json!("devex-clone-seed-source-registration")),
            ("post_copy", post_descriptor.clone()),
            ("post_verify", c29.clone()),
            ("source_request", source_descriptor.clone()),
            ("run_directory", json!(root.join(".local-tests/copy-run"))),
            ("run_manifest", manifest_descriptor),
            ("remote_writes", json!(0)),
            ("outbox_drained", json!(true)),
            ("restore_qualified", json!(false)),
        ] {
            registration[key] = value;
        }
        let registration_descriptor = write(
            &root,
            "copy-run/seed-runtime/source-registration.json",
            &registration,
        );
        let result = json!({"status":"seed_source_registered","registration":registration_descriptor,"source_request":source_descriptor,"source_storage":plan_descriptor,"generation_verified":plan_descriptor,"remote_writes":0,"outbox_drained":true,"restore_qualified":false});
        let outer = write(&root, "copy-run/results/0052.json", &result);
        write(
            &root,
            "copy-run/state.json",
            &json!({"attempts":[{"number":29,"stage":"post-copy","mode":"verify","status":"passed","result":c29}]}),
        );
        let (databases, inventory) = image_parts(&copy_plan, &plan, &objects);
        let image = json!({"format_version":1,"kind":"seed-source-generation-image","source_registration":outer,"image":{"databases":databases,"objects":inventory}});
        let image_descriptor = write(&root, "generation/before/image.json", &image);
        let source = json!({"review_successor":{"source_result":outer},"result":result,"registration":registration,"directory":root.join(".local-tests/copy-run"),"manifest":manifest,"request":source_request});
        let args = json!({"backend":root,"source":source,"post_request":post,"post_descriptor":post_descriptor,"copy_plan":copy_plan,"image_descriptor":image_descriptor,"verified_image":image});
        Self {
            _temp: temp,
            root,
            args,
            dataset,
            plan,
        }
    }
    fn derive(&self) -> Result<Value> {
        derive_with(&self.args, &|_, _, _| Ok(()))
    }
    fn image(&mut self, change: impl FnOnce(&mut Value)) {
        change(&mut self.args["verified_image"]);
        let path = PathBuf::from(self.args["image_descriptor"]["path"].as_str().unwrap());
        fs::remove_file(path).unwrap();
        self.args["image_descriptor"] = write(
            &self.root,
            "generation/before/image.json",
            &self.args["verified_image"],
        );
    }
}

#[test]
fn exact_lineage_retains_origin_ids_scale_placements_and_all_257_objects() {
    let f = Fixture::new();
    let value = f.derive().unwrap();
    assert_eq!(
        value["scale"],
        json!({"records":100000,"current_post_rows":100044,"tenants":11,"post_samples":33,"business_objects":256,"verified_objects":257,"object_bytes":1024*1024*1024})
    );
    assert_eq!(value["tenants"][1]["tenant_id"], "restore-source-unit-01");
    assert!(value["tenants"][1].get("password_env").is_none());
    assert_eq!(value["verification"]["request_interval_ms"], 1000);
    assert!(
        derive(&f.args).is_err(),
        "默认原生入口不得接受跳过完整复制计划验证的测试包装"
    );
    let calls = std::cell::Cell::new(0);
    assert!(
        derive_with(&f.args, &|_, _, _| {
            calls.set(calls.get() + 1);
            Err("plan-rejected".into())
        })
        .is_err()
    );
    assert_eq!(calls.get(), 1);
}

#[test]
fn unknown_placement_changed_digest_unknown_object_and_scope_are_rejected() {
    for scenario in 0..4 {
        let mut f = Fixture::new();
        f.image(|image| match scenario {
            0 => image["image"]["databases"]["shared"]["target"]["database"]["placements"]
                .as_array_mut()
                .unwrap()
                .push(json!({"tenant_id":"unknown"})),
            1 => {
                let key = image["image"]["objects"]["uploads"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .next()
                    .unwrap()
                    .clone();
                image["image"]["objects"]["uploads"][key]["sha256"] = json!("0".repeat(64));
            }
            2 => {
                image["image"]["objects"]["uploads"]["perf-seed-unit/unknown"] = json!({"bytes":1})
            }
            _ => image["source_registration"]["sha256"] = json!("0".repeat(64)),
        });
        assert!(f.derive().is_err(), "scenario {scenario}");
    }
}

#[test]
fn formal_dataset_requires_counts_sums_pacing_current_fields_and_unique_objects() {
    let f = Fixture::new();
    for (pointer, value) in [
        ("/records", json!(33)),
        ("/object_bytes", json!(1)),
        ("/request_interval_ms", json!(-1)),
        ("/tenants/0/records", json!(1)),
        ("/tenants/0/tenant_id", json!("unknown")),
        ("/tenants/0/files/0/sha256", json!("invalid")),
        ("/tenants/0/posts", json!([])),
        ("/tenants/0/files", json!([])),
    ] {
        let mut dataset = f.dataset.clone();
        *dataset.pointer_mut(pointer).unwrap() = value;
        let descriptor = write(
            &f.root,
            &format!("invalid-{}.json", uuid::Uuid::new_v4().simple()),
            &dataset,
        );
        let mut request = f.args["post_request"].clone();
        request["dataset"] = descriptor;
        let mut context = Context {
            root: f.root.clone(),
            snapshots: Vec::new(),
        };
        assert!(dataset::load(&mut context, &request).is_err(), "{pointer}");
    }
    let mut dataset = f.dataset.clone();
    dataset["tenants"][0]["password_env"] = json!("LEGACY");
    let descriptor = write(&f.root, "unexpected-password.json", &dataset);
    let mut request = f.args["post_request"].clone();
    request["dataset"] = descriptor;
    assert!(
        dataset::load(
            &mut Context {
                root: f.root,
                snapshots: Vec::new()
            },
            &request
        )
        .is_err()
    );
}

#[test]
fn exact_historical_exception_is_read_only_and_cannot_admit_other_content() {
    assert!(
        dataset::historical(
            7222,
            "7edf1f7fdff1aaa22dbe32ab524f42ad4425ddc7ad430dda833362a44ca719d5",
            "e2cd57cf6666cdef6ec40938e5d54d70e2168180783a718633e7b7639bb6ca75"
        )
        .is_ok()
    );
    for (bytes, sha, hash) in [
        (
            1,
            "7edf1f7fdff1aaa22dbe32ab524f42ad4425ddc7ad430dda833362a44ca719d5",
            "e2cd57cf6666cdef6ec40938e5d54d70e2168180783a718633e7b7639bb6ca75",
        ),
        (
            7222,
            "f",
            "e2cd57cf6666cdef6ec40938e5d54d70e2168180783a718633e7b7639bb6ca75",
        ),
        (
            7222,
            "7edf1f7fdff1aaa22dbe32ab524f42ad4425ddc7ad430dda833362a44ca719d5",
            "f",
        ),
    ] {
        assert!(dataset::historical(bytes, sha, hash).is_err());
    }
}

#[test]
fn lineage_rejects_ancestor_replacement_unknown_fields_and_changed_endpoints() {
    let mut f = Fixture::new();
    let mut value = f.derive().unwrap();
    value["unexpected"] = json!(true);
    let descriptor = write(&f.root, "lineage.json", &value);
    let mut request = f.args.clone();
    request["lineage_descriptor"] = descriptor;
    assert!(verify(&request).unwrap_err().to_string().contains("字段"));
    let mut unknown = f.args.clone();
    unknown["source"]["unknown"] = json!(true);
    assert!(
        derive_with(&unknown, &|_, _, _| Ok(()))
            .unwrap_err()
            .to_string()
            .contains("字段")
    );
    f.args["post_request"]["reference_plan"]["sha256"] = json!("0".repeat(64));
    assert!(f.derive().is_err());
    for endpoint in [
        "https://example.com:443",
        "http://127.0.0.1",
        "http://127.0.0.1:0",
        "http://@127.0.0.1:18210",
        "http://user:secret@127.0.0.1:18210",
        "http://127.0.0.1:18210/?query=1",
        "http://127.0.0.1:18210/#fragment",
    ] {
        assert!(chain::endpoint(&json!(endpoint)).is_err());
        assert!(chain::worker_endpoint(&json!(endpoint)).is_err());
    }
    for endpoint in [
        "http://127.0.0.1/readyz",
        "http://127.0.0.1:18211//readyz",
        "http://127.0.0.1:18211/other/../readyz",
        "http://127.0.0.1:18211/%2e/readyz",
        "http://127.0.0.1:18211/%72eadyz",
        "http://127.0.0.1:18211/readyz/",
        "http://127.0.0.1:18211/readyz?query=1",
        "http://127.0.0.1:18211/readyz#fragment",
        "http://127.0.0.1:18211/gateway/readyz",
    ] {
        assert!(chain::worker_endpoint(&json!(endpoint)).is_err());
    }
}

#[test]
fn reference_plan_reserves_readyz_for_worker_and_rejects_api_socket_overlap() {
    let f = Fixture::new();
    let load = |name: &str, plan: &Value| -> Result<Value> {
        let mut data = f.dataset.clone();
        data["plan_sha256"] = json!(digest::plan_hash(plan)?);
        let request = json!({
            "reference_plan": write(&f.root, &format!("{name}-plan.json"), plan),
            "dataset": write(&f.root, &format!("{name}-dataset.json"), &data),
        });
        dataset::load(
            &mut Context {
                root: f.root.clone(),
                snapshots: Vec::new(),
            },
            &request,
        )
        .map(|dataset| dataset.plan)
    };
    for (name, endpoint) in [
        ("implicit-root", "http://127.0.0.1:18211"),
        ("explicit-root", "http://127.0.0.1:18211/"),
    ] {
        let mut plan = f.plan.clone();
        plan["target"]["worker_ready_url"] = json!(endpoint);
        assert_eq!(load(name, &plan).unwrap(), plan);
    }
    for (index, field) in [
        "/source/api_url",
        "/target/api_url",
        "/target/frontend_url",
        "/source/s3/endpoint",
        "/target/s3/endpoint",
    ]
    .into_iter()
    .enumerate()
    {
        let mut plan = f.plan.clone();
        *plan.pointer_mut(field).unwrap() = json!("http://127.0.0.1:18211/readyz");
        assert_eq!(
            load(&format!("wrong-role-{index}"), &plan)
                .unwrap_err()
                .to_string(),
            "血缘端点必须是明确本机根地址"
        );
    }
    for (name, api, worker) in [
        (
            "overlap-ipv4-root",
            "http://127.0.0.1:18210/",
            "http://127.0.0.1:18210/",
        ),
        (
            "overlap-ipv4-readyz",
            "http://127.0.0.1:18210",
            "http://127.0.0.1:018210/readyz",
        ),
        (
            "overlap-ipv6-readyz",
            "https://[::1]:443/",
            "http://[0:0:0:0:0:0:0:1]:443/readyz",
        ),
    ] {
        let mut plan = f.plan.clone();
        plan["target"]["api_url"] = json!(api);
        plan["target"]["worker_ready_url"] = json!(worker);
        assert!(
            load(name, &plan)
                .unwrap_err()
                .to_string()
                .contains("探针不重叠")
        );
    }
}

#[test]
fn full_plan_rejects_same_physical_database_on_both_sides_and_unbound_inline_secrets() {
    let f = Fixture::new();
    for change in 0..4 {
        let mut plan = f.plan.clone();
        if change == 0 {
            plan["target"]["databases"][0] = plan["source"]["databases"][0].clone();
        } else if change == 1 {
            plan["source"]["s3"]["secret_key"] = json!("private-do-not-echo");
        } else if change == 2 {
            plan["target"]["databases"][0]["database"] =
                plan["source"]["databases"][0]["database"].clone();
        } else {
            plan["source"]["scope_id"] = json!("invalid-scope-");
        }
        let descriptor = write(&f.root, &format!("invalid-plan-{change}.json"), &plan);
        let mut request = f.args["post_request"].clone();
        request["reference_plan"] = descriptor;
        let error = dataset::load(
            &mut Context {
                root: f.root.clone(),
                snapshots: Vec::new(),
            },
            &request,
        )
        .err()
        .unwrap()
        .to_string();
        assert!(!error.contains("private-do-not-echo"));
    }
}

#[test]
fn revalidation_reconstructs_exact_lineage_and_late_ancestor_changes_fail_closed() {
    let f = Fixture::new();
    let value = f.derive().unwrap();
    let descriptor = write(&f.root, "lineage-exact.json", &value);
    let mut args = f.args.clone();
    args["lineage_descriptor"] = descriptor;
    assert_eq!(verify_with(&args, &|_, _, _| Ok(())).unwrap(), value);
    let mut overflow = value.clone();
    overflow["tenants"][0]["records"] = json!(u64::MAX);
    assert!(validate_lineage(&overflow).is_err());
    let mut changed = value;
    changed["scale"]["records"] = json!(33);
    args["lineage_descriptor"] = write(&f.root, "lineage-changed.json", &changed);
    assert!(verify_with(&args, &|_, _, _| Ok(())).is_err());
    let file = PathBuf::from(f.args["post_descriptor"]["path"].as_str().unwrap());
    assert!(
        derive_with(&f.args, &|_, _, _| {
            fs::write(&file, "{}")?;
            Ok(())
        })
        .unwrap_err()
        .to_string()
        .contains("变化")
    );
    assert_eq!(fs::read_to_string(file).unwrap(), "{}");
}

#[test]
fn same_database_names_on_different_servers_are_distinct_and_aliases_cannot_duplicate_targets() {
    let f = Fixture::new();
    let mut plan = f.plan.clone();
    for index in 0..4 {
        plan["target"]["databases"][index]["database"] =
            plan["source"]["databases"][index]["database"].clone();
    }
    let alternate = f.root.join(".local-tests/target-instance.cnf");
    let original = PathBuf::from(
        plan["target"]["databases"][0]["defaults_file"]
            .as_str()
            .unwrap(),
    );
    fs::write(
        &alternate,
        fs::read_to_string(original)
            .unwrap()
            .replace("port=3306", "port=3307"),
    )
    .unwrap();
    for db in plan["target"]["databases"].as_array_mut().unwrap() {
        db["defaults_file"] = json!(alternate);
        db["defaults_sha256"] = json!(digest::file(&alternate).unwrap().sha256);
    }
    let mut request = f.args["post_request"].clone();
    request["reference_plan"] = write(&f.root, "same-names-plan.json", &plan);
    let mut dataset = f.dataset.clone();
    dataset["plan_sha256"] = json!(digest::plan_hash(&plan).unwrap());
    request["dataset"] = write(&f.root, "same-names-dataset.json", &dataset);
    assert!(
        dataset::load(
            &mut Context {
                root: f.root.clone(),
                snapshots: Vec::new()
            },
            &request
        )
        .is_ok()
    );
    plan["target"]["databases"][1]["database"] = plan["target"]["databases"][0]["database"].clone();
    request["reference_plan"] = write(&f.root, "same-physical-alias.json", &plan);
    assert!(
        dataset::load(
            &mut Context {
                root: f.root,
                snapshots: Vec::new()
            },
            &request
        )
        .is_err()
    );
}

#[test]
fn full_business_inventory_can_have_no_probe_without_changing_the_business_mapping() {
    let f = Fixture::new();
    let mut copy = f.args["copy_plan"].clone();
    let mut image = f.args["verified_image"].clone();
    let historical = projection::objects(&f.dataset, &copy, &image).unwrap();
    let probe = copy["objects"].as_array_mut().unwrap().pop().unwrap();
    let key = probe["target_key"].as_str().unwrap();
    assert_eq!(historical["verified_objects"], 257);
    assert_eq!(historical["probe"]["target_key"], key);
    assert_eq!(historical["probe"]["sha256"], probe["artifact"]["sha256"]);
    assert!(projection::objects(&f.dataset, &copy, &image).is_err());
    image["image"]["objects"]["uploads"]
        .as_object_mut()
        .unwrap()
        .remove(key)
        .unwrap();
    let current = projection::objects(&f.dataset, &copy, &image).unwrap();
    assert_eq!(current["verified_objects"], 256);
    assert_eq!(current["probe"], Value::Null);
    for field in ["business_objects", "business_bytes", "mapping_sha256"] {
        assert_eq!(current[field], historical[field]);
    }
}

#[test]
fn a_second_fully_planned_probe_is_rejected_even_when_the_complete_inventory_matches() {
    let f = Fixture::new();
    let mut copy = f.args["copy_plan"].clone();
    let mut image = f.args["verified_image"].clone();
    let mut probe = copy["objects"].as_array().unwrap().last().unwrap().clone();
    probe["source_key"] = json!("restore-source-unit/system/second-probe.txt");
    probe["target_key"] = json!("perf-seed-unit/system/second-probe.txt");
    image["image"]["objects"]["uploads"]["perf-seed-unit/system/second-probe.txt"] =
        image["image"]["objects"]["uploads"]["perf-seed-unit/system/probe.txt"].clone();
    copy["objects"].as_array_mut().unwrap().push(probe);
    assert!(projection::objects(&f.dataset, &copy, &image).is_err());
}

#[test]
fn reference_resource_names_preserve_one_to_sixty_four_characters_without_clone_name_limits() {
    let f = Fixture::new();
    for length in [1, 64] {
        let mut plan = f.plan.clone();
        plan["id"] = json!("p".repeat(length));
        plan["source"]["databases"][0]["database"] = json!("d".repeat(length));
        let mut request = f.args["post_request"].clone();
        request["reference_plan"] = write(&f.root, &format!("names-{length}-plan.json"), &plan);
        let mut dataset = f.dataset.clone();
        dataset["plan_sha256"] = json!(digest::plan_hash(&plan).unwrap());
        request["dataset"] = write(&f.root, &format!("names-{length}-data.json"), &dataset);
        assert!(
            dataset::load(
                &mut Context {
                    root: f.root.clone(),
                    snapshots: Vec::new()
                },
                &request
            )
            .is_ok()
        );
    }
}
