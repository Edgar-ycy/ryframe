use std::{
    cell::Cell,
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{
        digest, json as json_io,
        lifecycle::stage::StageRequest,
        target_verify::{
            authority,
            input::{request, request_configuration},
            tree,
        },
    },
};

#[path = "target_verify_fixture_database.rs"]
pub(super) mod database;
#[path = "target_verify_fixture_history.rs"]
mod history;
#[path = "target_verify_fixture_observations.rs"]
pub(super) mod observations;
#[path = "target_verify_fixture_storage.rs"]
mod storage;

pub(super) const UUID: &str = "67cd8d4a-fe90-11ef-98fd-5847ca786499";
pub(super) const SCOPE: &str = "seed-scope";
pub(super) const KEYS: [&str; 4] = ["shared-control", "shared", "dedicated-a", "dedicated-b"];

pub(super) struct Fixture {
    _directory: tempfile::TempDir,
    pub(super) backend: PathBuf,
    pub(super) local: PathBuf,
    pub(super) target: PathBuf,
    pub(super) run: PathBuf,
    pub(super) request: Value,
    pub(super) environment: BTreeMap<String, String>,
}

impl Fixture {
    pub(super) fn new() -> Self {
        Self::build(false, false, "fixture")
    }

    pub(super) fn for_status() -> Self {
        let fixture = Self::build(true, false, "fixture");
        crate::recovery::clone::native::target_verify::published::unlocked_history(
            &fixture.backend,
            &fixture.target.join("initialized.json"),
        )
        .expect("status 夹具必须先满足完整 initialized/prepared 历史；错误仅报告模型前置原因");
        fixture
    }

    pub(super) fn for_initialization() -> Self {
        Self::build(true, true, "fixture")
    }

    pub(super) fn for_recovery() -> Self {
        Self::build(
            true,
            true,
            &format!("fixture-恢复窗口-中文 空格-{}", "e".repeat(120)),
        )
    }

    fn build(status_registration: bool, decoy: bool, workspace_name: &str) -> Self {
        let source = crate::workspace::root_dir();
        let parent = source.join(".local-tests");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("target-verify-native-")
            .tempdir_in(parent)
            .unwrap();
        let backend = directory.path().join("backend");
        let local = backend.join(".local-tests").join(workspace_name);
        fs::create_dir_all(&local).unwrap();
        source_repository(&source, &backend);
        fs::create_dir(local.join("target")).unwrap();
        fs::create_dir(local.join("run")).unwrap();
        let mut fixture = Self {
            _directory: directory,
            backend,
            target: local.join("target"),
            run: local.join("run"),
            local,
            request: Value::Null,
            environment: environment(),
        };
        fixture.write_request();
        if decoy {
            fs::create_dir(fixture.backend.join("config")).unwrap();
            fs::write(
                fixture.backend.join("config/app.toml"),
                "[invalid-default\n",
            )
            .unwrap();
            fs::write(
                fixture.backend.join("config/app.test.toml"),
                "# 明确诱饵，禁止默认目录回退\n",
            )
            .unwrap();
        }
        history::maintenance(&fixture);
        fixture.request["maintenance_build"] = bound(&fixture.local.join("maintenance/build.json"));
        fixture.seal_configuration();
        history::publish(&fixture, status_registration);
        fixture
    }

    fn write_request(&mut self) {
        for name in ["mysql", "rustfs", "wsl"] {
            fs::write(self.local.join(name), format!("explicit-mock-{name}")).unwrap();
        }
        let defaults = self.local.join("mysql.cnf");
        fs::write(&defaults, "[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=fixture-secret\nssl-mode=DISABLED\n").unwrap();
        let databases = request::KEYS.map(|(key, kind, mode)| json!({"key":key,"kind":kind,"mode":mode,"server_uuid":UUID,
            "database":database_name("seed",key),"defaults_file":defaults,"defaults_sha256":digest::file(&defaults).unwrap().sha256}));
        let tools = json!({"mysql": tool(&self.local.join("mysql"))});
        self.request = json!({"format_version":1,"kind":"devex-clone-fresh-target","id":"target-verify-fixture","side":"seed",
            "review":null,"maintenance_build":null,"configuration_sha256":"0".repeat(64),"tools":tools,
            "target":{"scope_id":SCOPE,"databases":databases,"s3":{"endpoint":"http://127.0.0.1:29000","region":"test-region",
                "access_key":"fixture-access","secret_key":"fixture-secret"}},
            "reset":{"legacy_ownership":{"mysql_exclusive":true,"redis_exclusive":true,"object_storage_exclusive":true},
                "credential_version":"fixture-v1","sentinel_key":format!("ryframe:devex-fresh:{SCOPE}:sentinel"),"sentinel_value":"devex-fresh:target-verify-fixture"},"storage":null});
        self.request["config_dir"] = json!(self.local.join("config"));
        let services = storage::files(self);
        let review = review(self, &services);
        write(&self.local.join("review.json"), &review);
        self.request["review"] = bound(&self.local.join("review.json"));
        self.request["review"]["canonical_sha256"] = json!(digest::plan_hash(&review).unwrap());
        self.write_configuration();
    }

    fn write_configuration(&self) {
        fs::create_dir(self.local.join("config")).unwrap();
        fs::write(
            self.local.join("config/app.test.toml"),
            "# 明确选择测试配置\n",
        )
        .unwrap();
        let targets = request::KEYS.iter().filter(|(key,_,_)| *key!="shared-control").map(|(key,_,mode)| json!({"key":key,"kind":"mysql","mode":mode,
            "host":"127.0.0.1","port":3306,"database":database_name("seed",key),"username":"fixture","password":"fixture-secret","tls_mode":"disabled"})).collect::<Vec<_>>();
        let table = json!({"scope_id":SCOPE,"bootstrap":{"admin_password":"Fixture!Admin123"},"jobs":{"mode":"external","health_host":"127.0.0.1","health_port":28081},"redis":{"host":"127.0.0.1","port":26379,"database":0,"tls":false,"password":"fixture-redis"},"app":{"host":"127.0.0.1","port":28080},"database":{"primary":{"host":"127.0.0.1","port":3306,
            "database":database_name("seed","shared-control"),"username":"fixture","password":"fixture-secret","tls_mode":"disabled"}},"tenant_data":{"targets":targets},
            "object_storage":{"backend":"rustfs","endpoint":"http://127.0.0.1:29000","region":"test-region","access_key":"fixture-access","secret_key":"fixture-secret","use_ssl":false},
            "reset":{"legacy_mysql_exclusive":true,"legacy_redis_exclusive":true,"legacy_object_storage_exclusive":true,"credential_version":"fixture-v1",
                "redis_outside_sentinel_key":self.request["reset"]["sentinel_key"],
                "services_stopped":true,"state_directory":self.target.join("reset-state"),
                "source_sha":crate::recovery::clone::native::source::binding_value(
                    &crate::recovery::clone::native::source::capture(&self.backend).unwrap().source)["snapshot"]["head"]}});
        fs::write(
            self.local.join("config/app.toml"),
            crate::target_configuration_fixture::toml_text(&table),
        )
        .unwrap();
    }

    fn seal_configuration(&mut self) {
        let (directory, files) = request_configuration::configuration_files(
            &self.backend,
            &self.environment,
            self.request.get("config_dir"),
        )
        .unwrap();
        self.request["configuration_sha256"] =
            json!(request_configuration::configuration_identity(&directory, &files).unwrap());
    }

    pub(super) fn stage(&self) -> StageRequest<'_> {
        StageRequest {
            backend: &self.backend,
            run_dir: &self.run,
            stage: "target-verify",
            mode: "run",
            attempt: Some(1),
        }
    }

    pub(super) fn authority(&self) -> Authority {
        Authority {
            calls: Cell::new(0),
        }
    }
    pub(super) fn storage_observer(&self) -> storage::ExternalFacts {
        storage::ExternalFacts::new(self)
    }
}

pub(super) struct Authority {
    pub(super) calls: Cell<usize>,
}

impl authority::Observer for Authority {
    fn inspect(&self, backend: &Path, stage: &StageRequest<'_>) -> Result<Value> {
        self.calls.set(self.calls.get() + 1);
        let manifest = json_io::read(&stage.run_dir.join("manifest.json"))?;
        let path = Path::new(
            manifest["initialized"]["path"]
                .as_str()
                .ok_or("fixture initialized path")?,
        );
        let target = path.parent().ok_or("fixture target parent")?;
        let initialized = json_io::read(path)?;
        let prepare = json_io::read(&target.join("prepare.json"))?;
        let mut generation = initialized["generation"].clone();
        generation
            .as_object_mut()
            .ok_or("fixture generation")?
            .remove("storage");
        let value = json!({"format_version":1,"kind":"devex-clone-target-authority","initialized":bound(path),"request":prepare["request"],
            "initialized_files":bound(&target.parent().ok_or("fixture workspace")?.join("initialized-files.json")),
            "files":tree::snapshot(backend,target,false)?,"generation":generation,"remote_writes":0,"target_ready":false,"clone_verified":false,"restore_qualified":false});
        authority::validate(backend, stage.run_dir, &value)?;
        Ok(value)
    }
}

fn environment() -> BTreeMap<String, String> {
    [
        ("APP_ENV", "test"),
        ("APP_SCOPE_ID", SCOPE),
        ("APP_JOBS_MODE", "external"),
        ("APP_JOBS_HEALTH_HOST", "127.0.0.1"),
        ("APP_JOBS_HEALTH_PORT", "28081"),
        ("APP_REDIS_HOST", "127.0.0.1"),
        ("APP_REDIS_PORT", "26379"),
        ("APP_REDIS_DATABASE", "0"),
        ("APP_REDIS_TLS", "false"),
        ("APP_REDIS_PASSWORD", "fixture-redis"),
        ("APP_TENANT_PASSWORD", "fixture-secret"),
        ("APP_OBJECT_STORAGE_ACCESS_KEY", "fixture-access"),
        ("APP_OBJECT_STORAGE_SECRET_KEY", "fixture-secret"),
        ("RYFRAME_RESET_ADMIN_PASSWORD", "Fixture!Admin123"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect()
}

fn review(fixture: &Fixture, services: &Value) -> Value {
    let mut scopes = serde_json::Map::new();
    for side in ["seed", "base", "candidate"] {
        let scope = format!("{side}-scope");
        let namespace = format!("ryframe:{{{scope}}}:");
        let databases=KEYS.map(|key|json!({"key":key,"database":database_name(side,key),"expected_server_uuid":UUID,"host":"127.0.0.1","port":3306,"connection_file":fixture.local.join("mysql.cnf")}));
        scopes.insert(side.into(),json!({"scope_id":scope,"runtime_dir":fixture.local.join(format!("{side}-runtime")),"identity_ledger":fixture.local.join(format!("{side}-identities")),
            "api_url":"http://127.0.0.1:28080","worker_ready_url":"http://127.0.0.1:28081/readyz","frontend_url":"http://127.0.0.1:28082",
            "objects":{"endpoint":"http://127.0.0.1:29000","region":"test-region"},"redis":{"url":"redis://127.0.0.1:26379/0","namespace":namespace,
            "ownership_key":format!("{namespace}.ryframe-owner"),"ownership_value":format!("ryframe-owner:v1:{scope}:redis")},"databases":databases}));
    }
    let mut tools = fixture.request["tools"].clone();
    tools["mysqldump"] = tools["mysql"].clone();
    tools["rustfs"] = tool(&fixture.local.join("rustfs"));
    tools["wsl"] = tool(&fixture.local.join("wsl"));
    tools["redis_server"] = json!({"distribution":"fixture","resolved_path":"/usr/bin/redis-server","sha256":"9".repeat(64)});
    json!({"kind":"review-only-perf-resource-plan-with-readonly-preflight","ready_for_execution":true,"scopes":scopes,"tools":tools,"services":services,
        "reference":{"source":{"databases":[{"server_uuid":UUID,"database":"protected_source"}]},"protected_target":{"databases":[{"server_uuid":UUID,"database":"protected_target"}]}}})
}

pub(super) fn database_name(side: &str, key: &str) -> String {
    format!("{side}_{}", key.replace('-', "_"))
}
pub(super) fn tool(path: &Path) -> Value {
    json!({"path":path,"sha256":digest::file(path).unwrap().sha256})
}
pub(super) fn bound(path: &Path) -> Value {
    let d = digest::file(path).unwrap();
    json!({"path":path,"bytes":d.bytes,"sha256":d.sha256})
}
pub(super) fn descriptor(path: &Path) -> Value {
    let d = digest::file(path).unwrap();
    json!({"bytes":d.bytes,"sha256":d.sha256})
}
pub(super) fn write(path: &Path, value: &Value) {
    json_io::write_new(path, value).unwrap();
}

pub(super) fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<(u64, String)>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                result.insert(path.strip_prefix(root).unwrap().to_path_buf(), None);
                pending.push(path);
            } else {
                let actual = digest::file(&path).unwrap();
                result.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    Some((actual.bytes, actual.sha256)),
                );
            }
        }
    }
    result
}

fn source_repository(source: &Path, backend: &Path) {
    fs::write(backend.join("Cargo.toml"), "[workspace]\nmembers=[]\n").unwrap();
    fs::write(backend.join(".gitignore"), "/.local-tests/\n/target/\n").unwrap();
    let product = backend.join("crates/ryframe");
    fs::create_dir_all(&product).unwrap();
    fs::write(
        product.join("Cargo.toml"),
        "[package]\nname=\"ryframe\"\nversion=\"0.0.0\"\nedition=\"2024\"\n",
    )
    .unwrap();
    for relative in [
        "sql/ryframe_config.sql",
        "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs",
    ] {
        let path = backend.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::copy(source.join(relative), path).unwrap();
    }
    let generated = "crates/ryframe-tenant-db/src/generated";
    copy_catalog_sources(&source.join(generated), &backend.join(generated));
    git(backend, &["init", "--quiet"]);
    git(
        backend,
        &["add", "--", "Cargo.toml", ".gitignore", "sql", "crates"],
    );
    git(
        backend,
        &[
            "-c",
            "user.name=RyFrame fixture",
            "-c",
            "user.email=fixture@ryframe.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    );
}

fn copy_catalog_sources(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            copy_catalog_sources(&path, &destination.join(entry.file_name()));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            fs::copy(path, destination.join(entry.file_name())).unwrap();
        }
    }
}

fn git(backend: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(backend)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "夹具 Git 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
