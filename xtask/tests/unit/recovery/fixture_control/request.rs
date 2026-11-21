use std::fs;

use super::*;
use crate::{
    cli::{FixtureFileOutput, FixtureSide},
    recovery::{
        clone::native::{lifecycle::state, resources::storage},
        fixture_control::{
            environment::protocol::tests as environment_fixture,
            source_pair::execution::tests::Fixture as Sources,
        },
    },
};

const SIDES: [FixtureSide; 3] = [FixtureSide::Seed, FixtureSide::Base, FixtureSide::Candidate];
const ACCESS: &str = "fixture-access";
const SECRET: &str = "fixture-secret";

struct RequestFixture {
    sources: Sources,
    execution: PathBuf,
    review: PathBuf,
    run: PathBuf,
    rustfs: Value,
}

impl RequestFixture {
    fn new() -> Self {
        let sources = Sources::new();
        let publication = sources.publication();
        let execution = PathBuf::from(
            publication["published"]["backend"]["path"]
                .as_str()
                .unwrap(),
        );
        let local = sources.backend.join(".local-tests");
        fs::create_dir(local.join("requests")).unwrap();
        let defaults = local.join("mysql.cnf");
        fs::write(&defaults, "[client]\nuser=fixture\n").unwrap();
        let tools = local.join("fixture-tool.exe");
        fs::write(&tools, "仅用于校验文件绑定").unwrap();
        let binding = json!({"path":tools,"sha256":digest::file(&tools).unwrap().sha256});
        let mut review = environment_fixture::review(&execution);
        for role in ["mysql", "mysqldump", "rustfs"] {
            review["tools"][role] = binding.clone();
        }
        for role in ["wsl", "redis_python"] {
            review["tools"].as_object_mut().unwrap().remove(role);
        }
        review["tools"]["redis_server"] = json!({
            "platform":"windows","path":"D:/fixture/redis-server.exe","sha256":"a".repeat(64),
            "version":"Redis server v=7.0.0 bits=64","dlls":{"fixture.dll":"b".repeat(64)}
        });
        review["services"]["rustfs"]["scope_id"] = json!("services-fixture-seed");
        for side in SIDES {
            for database in review["scopes"][side.as_str()]["databases"]
                .as_array_mut()
                .unwrap()
            {
                database["connection_file"] = json!(defaults);
            }
        }
        let review_path = local.join("review.json");
        document::write_new(&review_path, &review).unwrap();
        let run = Path::new(review["future_root"].as_str().unwrap()).join("service-run");
        fs::create_dir_all(&run).unwrap();
        let fixture = Self {
            sources,
            execution,
            review: review_path,
            run,
            rustfs: binding,
        };
        for side in SIDES {
            let name = side.as_str();
            fixture.prepare(
                side,
                &format!("access_key='{ACCESS}-{name}'\nsecret_key='{SECRET}-{name}'\n"),
            );
        }
        let seed = fixture.options(FixtureSide::Seed);
        document::write_new(&fixture.run.join("manifest.json"), &json!({
            "format_version":1,"kind":"reference-fixture-service-run",
            "review":file_binding(&fixture.review,"审阅").unwrap(),
            "bootstrap":file_binding(&seed.environment,"seed 环境").unwrap(),
            "execution_backend":fixture.execution,"scope_id":"services-fixture-seed","data_directory_was_empty":true
        })).unwrap();
        state::initialize(&fixture.run).unwrap();
        fixture
    }

    fn options(&self, side: FixtureSide) -> FixtureRequestOptions {
        let name = side.as_str();
        FixtureRequestOptions {
            environment: self
                .execution
                .join(format!(".local-tests/request-{name}/bootstrap.json")),
            service_run: self.run.clone(),
            id: format!("fresh-{name}"),
            side,
            output: FixtureFileOutput {
                output: self
                    .sources
                    .backend
                    .join(format!(".local-tests/requests/{name}.json")),
            },
        }
    }

    fn prepare(&self, side: FixtureSide, credentials: &str) {
        let options = self.options(side);
        let directory = options.environment.parent().unwrap();
        let config = directory.join("config");
        fs::create_dir_all(&config).unwrap();
        let name = side.as_str();
        fs::write(
            config.join("app.test.toml"),
            format!("scope_id='fixture-{name}'\n[reset]\ncredential_version='fixture-v1'\n[object_storage]\n{credentials}"),
        ).unwrap();
        let mut review = file_binding(&self.review, "审阅").unwrap();
        review["canonical_sha256"] =
            json!(digest::plan_hash(&document::read(&self.review).unwrap()).unwrap());
        let mut plan = json!({
            "kind":"reference-fixture-environment-plan","side":name,"scope_id":format!("fixture-{name}"),
            "review":review,"execution_source":self.sources.binding(),"maintenance_build":{"fixture":"build"}
        });
        plan["sha256"] = json!(digest::plan_hash(&plan).unwrap());
        document::write_replace(&directory.join("environment.json"), &json!({
            "environment":{"APP_ENV":"test","AWS_ACCESS_KEY_ID":"ambient-access","AWS_SECRET_ACCESS_KEY":"ambient-secret"}
        })).unwrap();
        document::write_replace(&options.environment, &json!({
            "kind":"reference-fixture-environment","status":"prepared","services_started":false,
            "remote_writes":0,"plan":plan,"execution_backend":self.execution,"config_dir":config,
            "configuration_sha256":configuration_digest(&config).unwrap()
        })).unwrap();
    }

    fn register_rustfs(&self) -> (Value, Value) {
        let rustfs = self.run.join("rustfs");
        let data = self.run.join("data");
        for path in [&rustfs, &data] {
            fs::create_dir(path).unwrap();
        }
        let (device, inode) =
            common::device_inode_for(&data, &fs::metadata(&data).unwrap()).unwrap();
        let scope = "services-fixture-seed";
        let executable = &self.rustfs["path"];
        let process = json!({"pid":10001,"started":"1","executable":executable});
        let supervisor = json!({"pid":10002,"started":"1","executable":executable});
        let monitor = json!({"pid":10003,"started":"1","executable":executable});
        let api = "http://127.0.0.1:29200";
        let console = "http://127.0.0.1:29201";
        let request = json!({
            "scope_id":scope,"executable":self.rustfs,"data_directory":{"path":data,"device":device,"inode":inode},
            "api_url":api,"console_url":console,"credential_files":{
                "access_key":{"path":data.join("access-key.txt")},"secret_key":{"path":data.join("secret-key.txt")}
            }
        });
        let arguments = storage::model::arguments(&request).unwrap();
        let request_binding = write_binding(&rustfs.join("request.json"), &request);
        let controller = write_binding(
            &self.run.join("controller-0001.json"),
            &json!({"fixture":true}),
        );
        let manifest = file_binding(&self.run.join("manifest.json"), "服务清单").unwrap();
        let intent = write_binding(
            &rustfs.join("intent.json"),
            &json!({
                "format_version":1,"kind":"devex-clone-storage-intent","request":manifest,
                "controller":controller,"attempt":1,"arguments":arguments
            }),
        );
        let process_receipt = write_binding(
            &rustfs.join("process.json"),
            &json!({
                "format_version":1,"role":"rustfs","scope_id":scope,"lifecycle":"running","identity":process,
                "api_url":api,"console_url":console,"data_dir":data,"intent":intent
            }),
        );
        let launch_receipt = write_binding(
            &rustfs.join("launch.json"),
            &json!({
                "format_version":1,"scope_id":scope,"identity":process,"arguments":arguments,"credential_files":request["credential_files"]
            }),
        );
        let tree = write_binding(
            &rustfs.join("rustfs-tree.json"),
            &json!({
                "format_version":2,"kind":"full-stack-process-tree","runtime_directory":rustfs,"role":"rustfs",
                "scope_id":scope,"operation_id":"b".repeat(32),"supervisor":supervisor,"process":process,
                "monitor":monitor,"group_id":supervisor["pid"]
            }),
        );
        let result = json!({
            "identity":process,"sha256":self.rustfs["sha256"],"process_receipt":process_receipt,
            "launch_receipt":launch_receipt,"tree":tree
        });
        (request_binding, result)
    }

    fn register_services(&self) {
        let (rustfs_request, rustfs_result) = self.register_rustfs();
        fs::create_dir(self.run.join("redis")).unwrap();
        let redis_request = write_binding(
            &self.run.join("redis/request.json"),
            &json!({"fixture":true}),
        );
        let runtime = json!({"redis":{"generation":"fixture-first-generation","port":26379}});
        write_binding(&self.run.join("redis/runtime.json"), &runtime);
        for (stage, mode, sources, result) in [
            (
                "storage-target",
                "initial",
                json!({"request":rustfs_request}),
                rustfs_result,
            ),
            (
                "cache-target",
                "initial",
                json!({"request":redis_request}),
                json!({"service":"redis","runtime":runtime}),
            ),
            (
                "fixture-buckets",
                "prepare",
                json!({}),
                json!({"status":"buckets_ready"}),
            ),
        ] {
            let (number, _) = state::begin(&self.run, stage, mode, &sources).unwrap();
            state::finish(&self.run, number, Some(&result), None).unwrap();
        }
    }

    fn reject(&self, options: FixtureRequestOptions, expected: &str) {
        let output = options.output.output.clone();
        let error = run(
            &self.sources.backend,
            &FixtureRequestCommand::Publish(options),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains(ACCESS) && !error.contains(SECRET));
        assert!(!output.exists());
    }
}

fn write_binding(path: &Path, value: &Value) -> Value {
    document::write_new(path, value).unwrap();
    file_binding(path, "夹具文件").unwrap()
}

#[test]
fn each_side_loads_its_real_configuration_and_the_same_registered_seed_services() {
    let fixture = RequestFixture::new();
    fixture.register_services();
    let backend = &fixture.sources.backend;
    let seed = fixture.options(FixtureSide::Seed);
    let original = Service::load(backend, &fixture.run, &fixture.review).unwrap();
    for side in SIDES {
        let options = fixture.options(side);
        let name = side.as_str();
        let product = Product::load(backend, &options.environment, name).unwrap();
        let service = load_service(backend, &fixture.run, &product.prepared, name).unwrap();
        assert_eq!(service.storage, original.storage);
        assert_eq!(
            service.context["sources"]["bootstrap"],
            file_binding(&seed.environment, "seed 环境").unwrap()
        );
        let value = product
            .request(&options.id, name, &service.storage)
            .unwrap();
        binding_request::validate(backend, &value, &Value::Null).unwrap();
        assert_eq!(value["side"], name);
        assert_eq!(value["target"]["scope_id"], format!("fixture-{name}"));
        assert_eq!(
            value["target"]["s3"]["access_key"],
            format!("{ACCESS}-{name}")
        );
        assert_eq!(
            value["target"]["s3"]["secret_key"],
            format!("{SECRET}-{name}")
        );
        assert_eq!(
            value["execution_backend"],
            json!({"path":fixture.execution,"execution_source":fixture.sources.binding()})
        );
        assert_eq!(
            value["storage"]["redis"]["generation"],
            "fixture-first-generation"
        );
        for (database, (key, _, _)) in value["target"]["databases"]
            .as_array()
            .unwrap()
            .iter()
            .zip(request::KEYS)
        {
            assert_eq!(database["key"], key);
            assert_eq!(
                database["database"],
                format!("fixture_{name}_{}", key.replace('-', "_"))
            );
        }
    }

    let redis = &original.registered["evidence"]["redis_runtime"];
    let path = common::bound_file(backend, redis, "当前服务运行收据").unwrap();
    fs::write(path, b"{\"runtime\":\"changed after registration\"}").unwrap();
    query::dispatch(
        backend,
        "service-guard",
        &json!({"value":original.context,"running":null}),
    )
    .unwrap();
    for role in ["process_receipt", "launch_receipt"] {
        common::bound_file(
            backend,
            &original.storage["rustfs"][role],
            "当前服务运行收据",
        )
        .unwrap();
    }
    let error = common::bound_file(backend, redis, "当前服务运行收据")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("当前服务运行收据与登记摘要不一致"),
        "{error}"
    );
}

#[test]
fn request_rejects_redis_runtime_receipt_drift_after_successful_observation() {
    let fixture = RequestFixture::new();
    fixture.register_services();
    let backend = &fixture.sources.backend;
    let service = Service::load(backend, &fixture.run, &fixture.review).unwrap();
    let output = fixture.options(FixtureSide::Seed).output.output;
    let redis = &service.registered["evidence"]["redis_runtime"];
    let runtime = common::bound_file(backend, redis, "当前服务运行收据").unwrap();
    let mut expected = crate::clone_native_fixture::snapshot(&fixture.run);
    let running = json!({"redis":"running","rustfs":"running","termination":null});

    service
        .observe_with(backend, |registered| {
            assert_eq!(registered, &service.registered);
            Ok(running.clone())
        })
        .unwrap();
    assert_eq!(
        crate::clone_native_fixture::snapshot(&fixture.run),
        expected
    );
    assert!(!output.exists());

    let original = &expected[&runtime];
    let changed = std::str::from_utf8(original)
        .unwrap()
        .replacen("fixture-first-generation", "Fixture-first-generation", 1)
        .into_bytes();
    assert_eq!(changed.len(), original.len());
    assert_ne!(&changed, original);

    let mut observations = 0;
    let error = publish(&output, || {
        service.observe_with(backend, |registered| {
            assert_eq!(registered, &service.registered);
            observations += 1;
            fs::write(&runtime, &changed)?;
            Ok(running.clone())
        })?;
        Ok(service.storage.clone())
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "当前服务运行收据与登记摘要不一致");
    assert_eq!(observations, 1);
    assert!(!output.exists());
    assert!(!fixture.run.join("run.lock").exists());

    *expected.get_mut(&runtime).unwrap() = changed;
    assert_eq!(
        crate::clone_native_fixture::snapshot(&fixture.run),
        expected
    );
}

#[test]
fn request_refuses_wrong_scope_and_credentials_before_creating_output() {
    let fixture = RequestFixture::new();
    fixture.register_services();
    let base = fixture.options(FixtureSide::Base);
    let candidate = fixture.options(FixtureSide::Candidate);
    let mut crossed = candidate.clone();
    crossed.environment = base.environment.clone();
    fixture.reject(crossed, "私有环境侧或计划摘要不同");
    let mut receipt = document::read(&candidate.environment).unwrap();
    let mut plan = receipt["plan"].as_object().unwrap().clone();
    plan.remove("sha256");
    plan.insert("scope_id".into(), json!("fixture-base"));
    let mut plan = Value::Object(plan);
    plan["sha256"] = json!(digest::plan_hash(&plan).unwrap());
    receipt["plan"] = plan;
    document::write_replace(&candidate.environment, &receipt).unwrap();
    fixture.reject(candidate, "私有环境执行工作树与指定审阅侧不符");

    for (credentials, expected) in [
        ("secret_key='fixture-secret'\n", "对象存储须显式配置认证"),
        ("access_key='fixture-access'\n", "对象存储须显式配置认证"),
        (
            "access_key=123\nsecret_key='fixture-secret'\n",
            "对象存储须显式配置认证",
        ),
        (
            "access_key='fixture-access'\nsecret_key=123\n",
            "对象存储须显式配置认证",
        ),
        (
            "access_key=''\nsecret_key='fixture-secret'\n",
            "配置缺少明确文本值",
        ),
        (
            "access_key='fixture-access'\nsecret_key=''\n",
            "配置缺少明确文本值",
        ),
        (
            "access_key_env='AWS_ACCESS_KEY_ID'\nsecret_key_env='AWS_SECRET_ACCESS_KEY'\n",
            "对象存储须显式配置认证",
        ),
    ] {
        fixture.prepare(FixtureSide::Base, credentials);
        fixture.reject(base.clone(), expected);
    }
}

#[test]
fn requests_preserve_the_closed_seed_ledger_and_refuse_a_misbound_seed_bootstrap() {
    let closed = RequestFixture::new();
    closed.register_services();
    crate::target_fixture_services::close(&closed.run);
    let before = fs::read(closed.run.join("state.json")).unwrap();
    let manifest_before = fs::read(closed.run.join("manifest.json")).unwrap();
    closed.reject(closed.options(FixtureSide::Base), "已关闭或外部终止");
    assert_eq!(fs::read(closed.run.join("state.json")).unwrap(), before);
    assert_eq!(
        fs::read(closed.run.join("manifest.json")).unwrap(),
        manifest_before
    );
    assert!(!closed.run.join("run.lock").exists());

    let foreign = RequestFixture::new();
    let path = foreign.run.join("manifest.json");
    let mut manifest = document::read(&path).unwrap();
    let environment = foreign.options(FixtureSide::Base).environment;
    manifest["bootstrap"] = file_binding(&environment, "另侧环境").unwrap();
    document::write_replace(&path, &manifest).unwrap();
    fs::remove_file(foreign.run.join("state.json")).unwrap();
    fs::remove_dir(foreign.run.join("results")).unwrap();
    state::initialize(&foreign.run).unwrap();
    let before = fs::read(foreign.run.join("state.json")).unwrap();
    foreign.reject(
        foreign.options(FixtureSide::Candidate),
        "私有环境侧或计划摘要不同",
    );
    assert_eq!(fs::read(foreign.run.join("state.json")).unwrap(), before);
    assert_eq!(document::read(&path).unwrap(), manifest);
}

#[test]
fn seed_bootstrap_requires_the_same_file_and_registered_bytes() {
    let backend = portable_path(&crate::workspace::root_dir().canonicalize().unwrap());
    let local = backend.join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(local).unwrap();
    let bootstrap = directory.path().join("bootstrap.json");
    fs::write(&bootstrap, b"{}").unwrap();
    let recorded = common::binding(&bootstrap).unwrap();
    let selected = file_binding(&bootstrap, "fixture").unwrap();
    require_seed_bootstrap(&backend, &selected, &recorded).unwrap();

    #[cfg(windows)]
    {
        let mixed = PathBuf::from(
            bootstrap
                .to_str()
                .unwrap()
                .replace(".local-tests\\", ".local-tests/"),
        );
        let mixed = file_binding(&mixed, "fixture").unwrap();
        assert_ne!(mixed["path"], recorded["path"]);
        require_seed_bootstrap(&backend, &mixed, &recorded).unwrap();
    }

    let copy = directory.path().join("copy.json");
    fs::copy(&bootstrap, &copy).unwrap();
    let copy = file_binding(&copy, "fixture").unwrap();
    assert!(require_seed_bootstrap(&backend, &copy, &recorded).is_err());

    let mut stale = selected;
    stale["sha256"] = json!("0".repeat(64));
    assert!(require_seed_bootstrap(&backend, &stale, &recorded).is_err());

    fs::write(&bootstrap, b"[]").unwrap();
    let changed = file_binding(&bootstrap, "fixture").unwrap();
    assert!(require_seed_bootstrap(&backend, &changed, &recorded).is_err());
}

#[test]
fn fresh_request_keeps_legacy_projection_and_rejects_different_rustfs_tool() {
    let backend = portable_path(&crate::workspace::root_dir().canonicalize().unwrap());
    let local = backend.join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(local).unwrap();
    let directory = portable_path(&directory.path().canonicalize().unwrap());
    let credentials = directory.join("mysql-client.cnf");
    fs::write(&credentials, b"fixture credential").unwrap();
    let selected = json!({
        "scope_id":"fixture-base-demo",
        "objects":{"endpoint":"http://127.0.0.1:9010","region":"local"},
        "databases":request::KEYS.iter().rev().map(|(key, _, _)| json!({
            "key":key,"database":format!("demo-{key}"),"expected_server_uuid":"server-id",
            "connection_file":credentials,
        })).collect::<Vec<_>>(),
    });
    let defaults_sha256 = digest::file(&credentials).unwrap().sha256;
    let databases = database_bindings(&backend, &selected).unwrap();
    let expected_databases = request::KEYS
        .iter()
        .map(|(key, kind, mode)| json!({
            "key":key,"kind":kind,"mode":mode,"database":format!("demo-{key}"),
            "server_uuid":"server-id","defaults_file":credentials,"defaults_sha256":defaults_sha256,
        }))
        .collect::<Vec<_>>();
    let review =
        json!({"path":"ready.json","bytes":1,"sha256":"review","canonical_sha256":"canonical"});
    let maintenance = json!({"path":"build.json","bytes":2,"sha256":"maintenance"});
    let publication = json!({"path":"execution-source.json","bytes":3,"sha256":"publication"});
    let mysql = json!({"path":"mysql.exe","sha256":"mysql"});
    let rustfs = json!({"identity":{"pid":123,"started":"42","executable":"rustfs.exe"},
        "sha256":"rustfs","process_receipt":{"path":"process.json"},"launch_receipt":{"path":"launch.json"}});
    let redis = json!({"platform":"windows","pid":456,"started":"43","run_id":"same-generation"});
    let storage = json!({"rustfs":rustfs,"redis":redis});
    let execution = directory.join("backend");
    let configuration = execution.join("environment/config");
    let product = Product {
        prepared: Prepared {
            path: directory.join("bootstrap.json"),
            binding: Value::Null,
            receipt: json!({"plan":{"review":review,"maintenance_build":maintenance,"execution_source":publication}}),
            review_path: directory.join("ready.json"),
            review: json!({"scopes":{"base":selected},"tools":{"mysql":mysql,
                "rustfs":{"path":"rustfs.exe","sha256":"rustfs"}}}),
        },
        execution: execution.clone(),
        environment: directory.join("environment.json"),
        environment_binding: Value::Null,
        configuration: configuration.clone(),
        configuration_sha256: "config-sha".into(),
        databases,
        settings: json!({"object_storage":{"access_key":"fixture-access","secret_key":"fixture-secret"},
            "reset":{"credential_version":"fixture-v1"}}),
    };
    let expected = json!({
        "format_version":1,"kind":"devex-clone-fresh-target","id":"request-base-demo",
        "review":review,"side":"base",
        "target":{"scope_id":"fixture-base-demo","s3":{
            "endpoint":"http://127.0.0.1:9010","region":"local","access_key":"fixture-access","secret_key":"fixture-secret"},
            "databases":expected_databases},
        "maintenance_build":maintenance,"configuration_sha256":"config-sha","config_dir":configuration,
        "tools":{"mysql":mysql},"storage":storage,
        "reset":{"legacy_ownership":{"mysql_exclusive":true,"redis_exclusive":true,"object_storage_exclusive":true},
            "credential_version":"fixture-v1","sentinel_key":"ryframe:devex-fresh:fixture-base-demo:sentinel",
            "sentinel_value":"devex-fresh:request-base-demo"},
        "execution_backend":{"execution_source":publication,"path":execution},
    });
    assert_eq!(
        product
            .request("request-base-demo", "base", &storage)
            .unwrap(),
        expected
    );
    let mut drifted = storage;
    drifted["rustfs"]["sha256"] = json!("different");
    assert!(
        product
            .request("request-base-demo", "base", &drifted)
            .is_err()
    );
}

#[test]
fn publish_refuses_input_drift_and_preserves_the_first_output() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("request.json");
    let original = json!({"storage":{"generation":"first"}});
    let changed = json!({"storage":{"generation":"second"}});
    let mut snapshots = [original.clone(), changed.clone()].into_iter();
    assert!(publish(&output, || Ok(snapshots.next().unwrap())).is_err());
    assert_eq!(document::read(&output).unwrap(), original);
    assert!(publish(&output, || Ok(changed.clone())).is_err());
    assert_eq!(document::read(&output).unwrap(), original);

    let next = directory.path().join("accepted.json");
    assert_eq!(publish(&next, || Ok(changed.clone())).unwrap(), changed);
    assert_eq!(document::read(&next).unwrap(), changed);
}

#[test]
fn product_guard_rechecks_actual_selected_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let directory = portable_path(&directory.path().canonicalize().unwrap());
    let bootstrap = directory.join("bootstrap.json");
    let review = directory.join("review.json");
    let environment = directory.join("environment.json");
    for path in [&bootstrap, &review, &environment] {
        fs::write(path, b"{}").unwrap();
    }
    let configuration = directory.join("config");
    fs::create_dir(&configuration).unwrap();
    let selected = configuration.join("app.test.toml");
    fs::write(&selected, b"scope_id = \"first\"\n").unwrap();
    let product = Product {
        prepared: Prepared {
            binding: file_binding(&bootstrap, "fixture").unwrap(),
            path: bootstrap,
            receipt: json!({"plan":{"review":file_binding(&review, "fixture").unwrap()}}),
            review_path: review,
            review: Value::Null,
        },
        execution: directory,
        environment_binding: file_binding(&environment, "fixture").unwrap(),
        environment,
        configuration_sha256: configuration_digest(&configuration).unwrap(),
        configuration,
        databases: vec![],
        settings: Value::Null,
    };
    product.check().unwrap();
    fs::write(selected, b"scope_id = \"second\"\n").unwrap();
    assert!(product.check().is_err());
}

#[test]
fn direct_toml_credentials_remain_required_with_ambient_process_values() {
    const CHILD: &str = "RYFRAME_FIXTURE_REQUEST_AMBIENT_TEST_CHILD";
    const AMBIENT: [(&str, &str); 2] = [
        ("AWS_ACCESS_KEY_ID", "ambient-access"),
        ("AWS_SECRET_ACCESS_KEY", "ambient-secret"),
    ];
    if std::env::var_os(CHILD).is_none() {
        let name = format!(
            "{}::direct_toml_credentials_remain_required_with_ambient_process_values",
            module_path!().split_once("::").unwrap().1
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env(CHILD, "1")
            .envs(AMBIENT)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    for (name, value) in AMBIENT {
        assert_eq!(std::env::var(name).unwrap(), value);
    }

    let fixture = RequestFixture::new();
    fixture.register_services();
    let options = fixture.options(FixtureSide::Base);
    let backend = &fixture.sources.backend;
    let product = Product::load(backend, &options.environment, "base").unwrap();
    let service = load_service(backend, &fixture.run, &product.prepared, "base").unwrap();
    let request = product
        .request(&options.id, "base", &service.storage)
        .unwrap();
    binding_request::validate(backend, &request, &Value::Null).unwrap();
    assert_eq!(
        request["target"]["s3"]["access_key"],
        format!("{ACCESS}-base")
    );
    assert_eq!(
        request["target"]["s3"]["secret_key"],
        format!("{SECRET}-base")
    );

    fixture.prepare(
        FixtureSide::Base,
        &format!("access_key_env='AWS_ACCESS_KEY_ID'\nsecret_key='{SECRET}-base'\n"),
    );
    fixture.reject(options, "对象存储须显式配置认证");
}

#[test]
fn rehashed_toml_with_another_scope_is_rejected_before_request_publication() {
    let fixture = RequestFixture::new();
    fixture.register_services();
    let options = fixture.options(FixtureSide::Candidate);
    Product::load(&fixture.sources.backend, &options.environment, "candidate").unwrap();

    let review = fs::read(&fixture.review).unwrap();
    let ledger = crate::clone_native_fixture::snapshot(&fixture.run);
    let mut receipt = document::read(&options.environment).unwrap();
    let plan = receipt["plan"].clone();
    let config = options.environment.parent().unwrap().join("config");
    let selected = config.join("app.test.toml");
    let original = fs::read_to_string(&selected).unwrap();
    let changed = original.replacen("scope_id='fixture-candidate'", "scope_id='fixture-base'", 1);
    assert_ne!(changed, original);
    fs::write(&selected, changed).unwrap();
    let rebound = json!(configuration_digest(&config).unwrap());
    assert_ne!(receipt["configuration_sha256"], rebound);
    receipt["configuration_sha256"] = rebound;
    document::write_replace(&options.environment, &receipt).unwrap();

    fixture.reject(options.clone(), "私有环境未绑定当前 scope 或配置摘要");
    assert_eq!(document::read(&options.environment).unwrap()["plan"], plan);
    assert_eq!(fs::read(&fixture.review).unwrap(), review);
    assert_eq!(crate::clone_native_fixture::snapshot(&fixture.run), ledger);
    assert!(!fixture.run.join("run.lock").exists());
}

#[test]
fn request_refuses_when_either_registered_service_is_stopped() {
    let fixture = RequestFixture::new();
    fixture.register_services();
    let backend = &fixture.sources.backend;
    let service = Service::load(backend, &fixture.run, &fixture.review).unwrap();
    let output = fixture.options(FixtureSide::Seed).output.output;
    let original = crate::clone_native_fixture::snapshot(&fixture.run);

    for (redis, rustfs) in [("stopped", "running"), ("running", "stopped")] {
        let mut observations = 0;
        let error = publish(&output, || {
            service.observe_with(backend, |registered| {
                assert_eq!(registered, &service.registered);
                observations += 1;
                Ok(json!({"redis":redis,"rustfs":rustfs,"termination":null}))
            })?;
            Ok(service.storage.clone())
        })
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "fresh-target 请求签发时 RustFS 或 Redis 未保持存活",
            "Redis={redis}, RustFS={rustfs}"
        );
        assert_eq!(observations, 1);
        assert!(!output.exists());
        assert!(!fixture.run.join("run.lock").exists());
        assert_eq!(
            crate::clone_native_fixture::snapshot(&fixture.run),
            original
        );
    }
}
