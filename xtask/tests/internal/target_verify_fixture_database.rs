use std::{cell::Cell, collections::BTreeMap, fs, path::Path, process::ExitStatus, time::Duration};

use serde_json::{Value, json};

use super::{Fixture, KEYS, SCOPE, UUID, bound, descriptor, write};
use crate::{
    Result,
    recovery::clone::native::{
        catalog, digest,
        export::transport::{CommandOutput, CommandSpec, StdoutTarget, Transport},
        json as json_io,
        target_verify::database::model,
    },
};

pub(in super::super) struct FakeTransport<'a> {
    fixture: &'a Fixture,
    drift: bool,
    fail: bool,
    pub(crate) inventories: Cell<usize>,
    pub(crate) identities: Cell<usize>,
    pub(crate) ownerships: Cell<usize>,
}

impl<'a> FakeTransport<'a> {
    pub(in super::super) fn new(fixture: &'a Fixture, drift: bool, fail: bool) -> Self {
        Self {
            fixture,
            drift,
            fail,
            inventories: Cell::new(0),
            identities: Cell::new(0),
            ownerships: Cell::new(0),
        }
    }

    fn inventory(&self, spec: &CommandSpec) -> Result<CommandOutput> {
        self.inventories.set(self.inventories.get() + 1);
        if self.fail {
            return Ok(output(1, Vec::new()));
        }
        let key = argument(&spec.args, "--target")?;
        let path = Path::new(argument(&spec.args, "--output")?);
        let mut value = raw(self.fixture, key);
        if self.drift && self.inventories.get() == 5 {
            value["target"]["database"]["tables"][0]["sha256"] = json!("f".repeat(64));
        }
        json_io::write_new(path, &value)?;
        Ok(output(0, Vec::new()))
    }

    fn mysql(&self, spec: &CommandSpec) -> Result<CommandOutput> {
        let database = spec
            .args
            .iter()
            .find_map(|value| value.strip_prefix("--database="))
            .ok_or("fixture MySQL database")?;
        let declared = self.fixture.request["target"]["databases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|db| db["database"] == database)
            .ok_or("fixture MySQL unknown target")?;
        let query = argument(&spec.args, "--execute")?;
        let stdout = match query {
            "SELECT @@server_uuid, DATABASE();" => {
                self.identities.set(self.identities.get() + 1);
                format!("{UUID}\t{database}\n")
            }
            "SELECT resource_kind, scope_id, marker FROM ryframe_resource_ownership ORDER BY resource_kind;" =>
            {
                self.ownerships.set(self.ownerships.get() + 1);
                owner_rows(declared["kind"] == "combined")
            }
            _ => return Err("fixture 不支持非固定只读 MySQL 查询".into()),
        };
        Ok(output(0, stdout.into_bytes()))
    }
}

impl Transport for FakeTransport<'_> {
    fn s3(&self, request: crate::recovery::clone::native::s3::Request) -> Result<Value> {
        use crate::recovery::clone::native::s3::Operation;
        let bucket = request.bucket.as_str();
        assert!(["avatar", "config-packages", "exports", "imports", "uploads"].contains(&bucket));
        Ok(match request.operation {
            Operation::HeadBucket | Operation::Versioning => json!({}),
            Operation::List {
                prefix,
                continuation,
            } => {
                assert_eq!(prefix, format!("{SCOPE}/"));
                assert!(continuation.is_none());
                json!({"IsTruncated":false,"Contents":[{"Key":format!("{SCOPE}/.ryframe-owner")}]})
            }
            Operation::Get {
                destination, key, ..
            } => {
                assert!(destination.starts_with(&self.fixture.local));
                assert_eq!(key, format!("{SCOPE}/.ryframe-owner"));
                let mut file = fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(destination)?;
                std::io::Write::write_all(&mut file, owner(bucket).as_bytes())?;
                file.sync_all()?;
                json!({})
            }
            _ => return Err("fixture 不支持非固定只读对象操作".into()),
        })
    }

    fn run(&self, spec: CommandSpec) -> Result<CommandOutput> {
        assert_eq!(spec.cwd, self.fixture.backend);
        assert!(spec.stdin.is_none());
        assert!(spec.work_dir.starts_with(&self.fixture.local));
        let mut result = if spec
            .args
            .first()
            .is_some_and(|arg| arg == "target-inventory")
        {
            self.inventory(&spec)
        } else if spec.program
            == Path::new(
                self.fixture.request["tools"]["mysql"]["path"]
                    .as_str()
                    .unwrap(),
            )
        {
            self.mysql(&spec)
        } else {
            Err("fixture 外部命令不在已声明替身内".into())
        }?;
        match &spec.stdout {
            StdoutTarget::Capture => {}
            StdoutTarget::File(path) => {
                assert!(path.starts_with(&self.fixture.local));
                let mut file = fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(path)?;
                std::io::Write::write_all(&mut file, &result.stdout)?;
                file.sync_all()?;
                result.stdout.clear();
            }
            StdoutTarget::HeldFile { path, file } => {
                assert!(path.starts_with(&self.fixture.local));
                let mut output = file.try_clone()?;
                std::io::Write::write_all(&mut output, &result.stdout)?;
                output.sync_all()?;
                result.stdout.clear();
            }
        }
        Ok(result)
    }
}

fn argument<'a>(args: &'a [String], name: &str) -> Result<&'a str> {
    args.iter()
        .position(|value| value == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
        .ok_or_else(|| format!("fixture 缺少参数 {name}").into())
}

fn output(code: i32, stdout: Vec<u8>) -> CommandOutput {
    CommandOutput {
        status: status(code),
        stdout,
        stderr: Vec::new(),
    }
}
#[cfg(windows)]
fn status(code: i32) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(code as u32)
}
#[cfg(unix)]
fn status(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(code << 8)
}

fn raw(fixture: &Fixture, key: &str) -> Value {
    let db = fixture.request["target"]["databases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|db| db["key"] == key)
        .unwrap();
    let combined = db["kind"] == "combined";
    let (_, all, kept) = model::table_sets(&fixture.backend, combined).unwrap();
    let table = |name: &String| {
        let rows = if name == "ryframe_resource_ownership" {
            if combined { 2 } else { 1 }
        } else if kept.contains(name) || name == "biz_tenant_target_slot" {
            1
        } else {
            0
        };
        json!({"table":name,"rows":rows,"sha256":digest::plan_hash(&json!({"table":name,"rows":rows})).unwrap()})
    };
    let loaded = catalog::load(&fixture.backend).unwrap();
    assert!(loaded.tenant.keys().all(|name| all.contains(name)));
    json!({"scope_id":SCOPE,"control_schema_fingerprint":"c".repeat(16),"tenant_schema_fingerprint":"d".repeat(64),"target":{
        "database":{"key":key,"kind":db["kind"],"server_uuid":UUID,"database":db["database"],"shared":db["mode"]=="shared","placements":[],
            "tables":all.difference(&kept).map(table).collect::<Vec<_>>()},"preserved_tables":kept.iter().map(table).collect::<Vec<_>>()}})
}

fn owner_rows(combined: bool) -> String {
    let mut rows = String::new();
    if combined {
        rows.push_str(&format!(
            "control\t{SCOPE}\tryframe-owner:v1:{SCOPE}:control\n"
        ));
    }
    rows.push_str(&format!(
        "tenant-data\t{SCOPE}\tryframe-owner:v1:{SCOPE}:tenant-data\n"
    ));
    rows
}

pub(super) fn initial(fixture: &Fixture, generation: &Value) -> Value {
    let directory = fixture.target.join("inventory-initial");
    fs::create_dir(&directory).unwrap();
    let transport = FakeTransport::new(fixture, false, false);
    let binding = inventory_binding(fixture, generation);
    write(&directory.join("binding-before.json"), &binding);
    let mut inventories = BTreeMap::new();
    let mut observed = BTreeMap::new();
    for phase in ["before", "after"] {
        verify_databases(fixture, &directory, &transport);
        for key in KEYS {
            let name = format!("{phase}-target-{key}.json");
            let path = directory.join(&name);
            let (value, image) = inventory_target(fixture, &transport, generation, key, &path);
            if phase == "after" {
                assert_eq!(
                    json_io::read(&directory.join(format!("before-target-{key}.json"))).unwrap(),
                    value
                );
                assert_eq!(observed[key], image);
            }
            observed.insert(key.to_owned(), image);
            inventories.insert(name, descriptor(&path));
        }
        verify_databases(fixture, &directory, &transport);
    }
    assert_eq!(transport.inventories.get(), 8);
    assert_eq!(transport.identities.get(), 16);
    assert_eq!(transport.ownerships.get(), 24);
    write(&directory.join("binding-after.json"), &binding);
    let receipt = json!({"format_version":1,"status":"side_inventory_captured","side":"target","scope_id":SCOPE,"binding_sha256":digest::plan_hash(&binding).unwrap(),
        "keys":KEYS,"inventories":inventories,"observations":observed,"remote_writes":0,"producer_stopped_proven":false,"fresh_target_proven":false,"target_ready":false,"clone_verified":false,"restore_qualified":false});
    write(&directory.join("inventory.json"), &receipt);
    let images = observed
        .values()
        .map(|image| {
            (
                image["resource"]["database"].as_str().unwrap().to_owned(),
                image.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(images.len(), 4);
    json!({"receipt":bound(&directory.join("inventory.json")),"observations":images})
}

fn inventory_binding(fixture: &Fixture, generation: &Value) -> Value {
    let mut target = fixture.request["target"].clone();
    for field in ["runtime_dir", "api_url"] {
        target[field] = generation["selected"][field].clone();
    }
    json!({"side":"target","tools_plan_sha256":digest::plan_hash(&json!({"target":target,"tools":fixture.request["tools"]})).unwrap(),
        "configuration":generation["configuration"],"physical":generation["physical"],"maintenance":descriptor(&fixture.local.join("maintenance/build.json")),"source":generation["maintenance"]["source"]})
}

fn inventory_target(
    fixture: &Fixture,
    transport: &FakeTransport<'_>,
    generation: &Value,
    key: &str,
    path: &Path,
) -> (Value, Value) {
    let directory = path.parent().unwrap();
    let executable = generation["maintenance"]["artifacts"]["tenant-data"]["executable"]
        .as_str()
        .unwrap()
        .into();
    command(
        fixture,
        directory,
        transport,
        executable,
        vec![
            "target-inventory".into(),
            "--target".into(),
            key.into(),
            "--output".into(),
            path.display().to_string(),
        ],
    );
    let db = fixture.request["target"]["databases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|db| db["key"] == key)
        .unwrap();
    let sql = "SELECT resource_kind, scope_id, marker FROM ryframe_resource_ownership ORDER BY resource_kind;";
    let response = command(
        fixture,
        directory,
        transport,
        fixture.request["tools"]["mysql"]["path"]
            .as_str()
            .unwrap()
            .into(),
        mysql_arguments(db, sql),
    );
    let owners = model::owners(std::str::from_utf8(&response.stdout).unwrap().trim()).unwrap();
    let value = json_io::read(path).unwrap();
    let image = model::observation(&fixture.backend, &value, db, SCOPE, &owners).unwrap();
    (value, image)
}

fn mysql_arguments(db: &Value, sql: &str) -> Vec<String> {
    vec![
        format!("--defaults-file={}", db["defaults_file"].as_str().unwrap()),
        format!("--database={}", db["database"].as_str().unwrap()),
        "--batch".into(),
        "--raw".into(),
        "--skip-column-names".into(),
        "--binary-mode".into(),
        "--default-character-set=utf8mb4".into(),
        "--execute".into(),
        sql.into(),
    ]
}

fn verify_databases(fixture: &Fixture, directory: &Path, transport: &FakeTransport<'_>) {
    for db in fixture.request["target"]["databases"].as_array().unwrap() {
        for check in ["identity", "ownership"] {
            let sql = if check == "identity" {
                "SELECT @@server_uuid, DATABASE();"
            } else {
                "SELECT resource_kind, scope_id, marker FROM ryframe_resource_ownership ORDER BY resource_kind;"
            };
            let result = command(
                fixture,
                directory,
                transport,
                fixture.request["tools"]["mysql"]["path"]
                    .as_str()
                    .unwrap()
                    .into(),
                vec![
                    format!("--defaults-file={}", db["defaults_file"].as_str().unwrap()),
                    format!("--database={}", db["database"].as_str().unwrap()),
                    "--batch".into(),
                    "--raw".into(),
                    "--skip-column-names".into(),
                    "--binary-mode".into(),
                    "--default-character-set=utf8mb4".into(),
                    "--execute".into(),
                    sql.into(),
                ],
            );
            let response = std::str::from_utf8(&result.stdout).unwrap().trim();
            if check == "identity" {
                assert_eq!(
                    response,
                    format!("{UUID}\t{}", db["database"].as_str().unwrap())
                );
            } else {
                assert_eq!(
                    model::owners(response).unwrap(),
                    model::expected_owners(SCOPE, db["kind"] == "combined")
                );
            }
        }
    }
}

fn command(
    fixture: &Fixture,
    directory: &Path,
    transport: &FakeTransport<'_>,
    program: std::path::PathBuf,
    args: Vec<String>,
) -> CommandOutput {
    let mut argv = vec![program.display().to_string()];
    argv.extend(args.clone());
    let output = transport
        .run(CommandSpec {
            program,
            args,
            cwd: fixture.backend.clone(),
            environment: fixture.environment.clone(),
            stdin: None,
            stdout: StdoutTarget::Capture,
            work_dir: directory.to_path_buf(),
            label: "fixture-initial-inventory".into(),
            timeout: Duration::from_secs(30),
        })
        .unwrap();
    assert!(output.status.success());
    let name = format!("command-{}.json", uuid::Uuid::new_v4().simple());
    write(
        &directory.join(name),
        &json!({"command":argv,"returncode":output.status.code(),"error_type":null,"stdin":null,
        "stdout":std::str::from_utf8(&output.stdout).unwrap(),"stderr":std::str::from_utf8(&output.stderr).unwrap()}),
    );
    output
}

fn owner(bucket: &str) -> String {
    format!("ryframe-owner:v1:{SCOPE}:object-storage:{bucket}")
}
pub(super) fn objects() -> Value {
    use sha2::{Digest, Sha256};
    let result = ["avatar", "config-packages", "exports", "imports", "uploads"].map(|bucket| {
        let data = owner(bucket);
        (
            bucket,
            json!({"keys":[format!("{SCOPE}/.ryframe-owner")],
        "owner":{"bytes":data.len(),"sha256":hex::encode(Sha256::digest(data.as_bytes()))}}),
        )
    });
    serde_json::to_value(result.into_iter().collect::<BTreeMap<_, _>>()).unwrap()
}
