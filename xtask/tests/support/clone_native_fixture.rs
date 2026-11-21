// 离线复制测试共享的文件与源码夹具。
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::workspace;

static NEXT: AtomicUsize = AtomicUsize::new(1);
const UUID: &str = "67cd8d4a-fe90-11ef-98fd-5847ca786499";
pub type TestRow = Map<String, Value>;
pub type TestData = BTreeMap<String, Vec<(String, TestRow)>>;

pub struct Fixture {
    pub(crate) root: PathBuf,
    pub(crate) export: PathBuf,
    pub input: PathBuf,
    pub(crate) value: Value,
    pub data: TestData,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let (root, export, input) = fixture_paths();
        copy_catalog(&root);
        let evidence = initial_evidence(&export);
        let objects = initial_objects(&export);
        let mut data = BTreeMap::new();
        data.insert("shared-control".to_owned(), Vec::new());
        data.insert("dedicated-a".to_owned(), Vec::new());
        let mut fixture = Self {
            root,
            export,
            input,
            value: json!({
                "format_version": 1,
                "copy_id": "clone-case",
                "copy_stage": "source_to_seed",
                "target_schedule_actions": [],
                "app_env": "test",
                "artifact_root": "",
                "execution_backend": null,
                "source_snapshot": {
                    "head": "a".repeat(40),
                    "clean": false,
                    "worktree_sha256": "b".repeat(64),
                },
                "source": side("clone-source"),
                "target": {
                    "scope_id": "clone-target",
                    "object_endpoint": "http://127.0.0.1:29200",
                    "databases": side("clone-target")["databases"].clone(),
                    "state": "new_initialized",
                    "ever_started": false,
                },
                "evidence": evidence,
                "databases": [],
                "objects": objects,
            }),
            data,
        };
        fixture.value["artifact_root"] = json!(fixture.export);
        seed_initial_rows(&mut fixture);
        fixture.refresh();
        fixture
    }

    pub fn add(
        &mut self,
        key: &str,
        table: &str,
        values: &[(&str, Value)],
    ) -> &mut Map<String, Value> {
        let templates = self.templates(key, table);
        let mut row = templates
            .into_iter()
            .map(|name| (name, Value::Null))
            .collect::<Map<_, _>>();
        for (name, value) in values {
            row.insert((*name).into(), value.clone());
        }
        let rows = self.data.get_mut(key).unwrap();
        rows.push((table.to_owned(), row));
        let (_, row) = rows.last_mut().unwrap();
        row
    }

    pub fn refresh(&mut self) {
        let control =
            table_columns(&fs::read_to_string(self.root.join("sql/ryframe_config.sql")).unwrap());
        let baseline = table_columns(
            &fs::read_to_string(self.root.join(
                "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs",
            ))
            .unwrap(),
        );
        let mut dumps = Vec::new();
        for (key, rows) in &self.data {
            let mut tables = BTreeMap::new();
            if key == "shared-control" {
                for table in control.keys() {
                    if ![
                        "ryframe_resource_ownership",
                        "sys_backup_set",
                        "sys_backup_resource",
                        "sys_restore_run",
                        "sys_tenant_data_backup_point",
                        "seaql_migrations",
                    ]
                    .contains(&table.as_str())
                    {
                        tables.insert(table.clone(), 0_u64);
                    }
                }
                for table in baseline
                    .keys()
                    .filter(|table| table.as_str() != "ryframe_resource_ownership")
                {
                    tables.insert(table.clone(), 0);
                }
            } else {
                for table in baseline
                    .keys()
                    .filter(|table| table.as_str() != "ryframe_resource_ownership")
                {
                    tables.insert(table.clone(), 0);
                }
            }
            let mut content = String::from("-- ryframe-reference-data-v1\n");
            for (table, row) in rows {
                *tables.get_mut(table).unwrap() += 1;
                content.push_str(&format!(
                    "INSERT INTO `{table}` ({}) VALUES ({});\n",
                    row.keys()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    row.values().map(sql_literal).collect::<Vec<_>>().join(",")
                ));
            }
            dumps.push(json!({
                "key": key,
                "tables": tables,
                "artifact": artifact(&self.export, &format!("{key}.sql"), content.as_bytes()),
            }));
        }
        self.value["databases"] = Value::Array(dumps);
    }

    fn templates(&self, key: &str, table: &str) -> Vec<String> {
        let mut result =
            table_columns(&fs::read_to_string(self.root.join("sql/ryframe_config.sql")).unwrap());
        if key != "shared-control" {
            result = BTreeMap::new();
        }
        if key == "shared-control" || matches!(table, "biz_tenant_fence" | "biz_tenant_target_slot")
        {
            let baseline = table_columns(
                &fs::read_to_string(self.root.join(
                    "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs",
                ))
                .unwrap(),
            );
            result.extend(baseline);
        }
        result.remove(table).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.root
                .starts_with(workspace::root_dir().join(".local-tests"))
        );
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn fixture_paths() -> (PathBuf, PathBuf, PathBuf) {
    let root = workspace::root_dir().join(format!(
        ".local-tests/clone-native-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let export = root.join(".local-tests/export");
    fs::create_dir_all(&export).unwrap();
    let input = root.join(".local-tests/input.json");
    (root, export, input)
}

fn copy_catalog(root: &Path) {
    for relative in [
        "sql/ryframe_config.sql",
        "crates/ryframe-tenant-db/src/generated/catalog.rs",
        "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs",
    ] {
        let target = root.join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(workspace::root_dir().join(relative), &target).unwrap();
    }
}

fn initial_evidence(export: &Path) -> Map<String, Value> {
    ["source_stopped", "target_initialized", "target_stopped"]
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                artifact(export, &format!("{name}.json"), b"{}"),
            )
        })
        .collect()
}

fn initial_objects(export: &Path) -> Vec<Value> {
    ["avatar", "config-packages", "exports", "imports", "uploads"]
        .into_iter()
        .map(|bucket| {
            let entries = if bucket == "uploads" {
                vec![json!({
                    "key": "clone-source/system/file.txt",
                    "artifact": artifact(export, "file.bin", b"data"),
                    "metadata": metadata(),
                })]
            } else {
                Vec::new()
            };
            json!({"bucket": bucket, "entries": entries})
        })
        .collect()
}

fn seed_initial_rows(fixture: &mut Fixture) {
    for (tenant, target) in [
        ("system", "shared-control"),
        ("clone-source-01", "dedicated-a"),
    ] {
        fixture.add(
            "shared-control",
            "sys_tenant",
            &[
                ("id", json!(if tenant == "system" { 1 } else { 2 })),
                ("tenant_id", json!(tenant)),
                ("name", json!(tenant)),
                ("status", json!("enabled")),
            ],
        );
        fixture.add(
            "shared-control",
            "sys_tenant_data_placement",
            &[
                ("tenant_id", json!(tenant)),
                ("current_target_key", json!(target)),
                ("placement_generation", json!(1)),
                ("switch_token", json!("token")),
                ("state", json!("active")),
            ],
        );
        fixture.add(
            target,
            "biz_tenant_fence",
            &[
                ("tenant_id", json!(tenant)),
                ("target_key", json!(target)),
                ("placement_generation", json!(1)),
                ("switch_token", json!("token")),
                ("state", json!("active")),
            ],
        );
    }
    fixture.add(
        "shared-control",
        "biz_tenant_target_slot",
        &[("slot_id", json!(1))],
    );
    fixture.add(
        "dedicated-a",
        "biz_tenant_target_slot",
        &[
            ("slot_id", json!(1)),
            ("tenant_id", json!("clone-source-01")),
            ("placement_generation", json!(1)),
            ("switch_token", json!("token")),
        ],
    );
    fixture.add(
        "shared-control",
        "sys_file",
        &[
            ("id", json!(1)),
            ("tenant_id", json!("system")),
            ("bucket", json!("uploads")),
            ("storage_path", json!("system/file.txt")),
            ("file_url", json!("uploads/system/file.txt")),
            ("file_size", json!(4)),
            ("file_sha256", json!(sha(b"data"))),
            ("upload_status", json!("ready")),
            ("del_flag", json!("0")),
        ],
    );
}

fn side(scope: &str) -> Value {
    let databases = BTreeMap::from([
        ("shared-control", ("combined", "shared")),
        ("dedicated-a", ("tenant", "dedicated")),
    ])
    .into_iter()
    .map(|(key, (kind, mode))| {
        let kinds = if kind == "combined" {
            &["control", "tenant-data"][..]
        } else {
            &["tenant-data"][..]
        };
        json!({
            "key": key,
            "kind": kind,
            "mode": mode,
            "server_uuid": UUID,
            "database": format!("{}_{}", scope.replace('-', "_"), key.replace('-', "_")),
            "schema_sha256": "c".repeat(64),
            "ownership": kinds.iter().map(|resource| json!({
                "resource_kind": resource,
                "scope_id": scope,
                "marker": format!("ryframe-owner:v1:{scope}:{resource}"),
            })).collect::<Vec<_>>(),
        })
    })
    .collect::<Vec<_>>();
    json!({
        "scope_id": scope,
        "object_endpoint": "http://127.0.0.1:29200",
        "databases": databases,
    })
}

fn artifact(root: &Path, name: &str, content: &[u8]) -> Value {
    let path = root.join(name);
    fs::write(&path, content).unwrap();
    json!({
        "file": name,
        "bytes": content.len(),
        "sha256": sha(content),
    })
}

fn metadata() -> Value {
    json!({
        "ContentType": "text/plain",
        "CacheControl": null,
        "ContentDisposition": null,
        "ContentEncoding": null,
        "ContentLanguage": null,
        "Expires": null,
        "WebsiteRedirectLocation": null,
        "Metadata": {},
    })
}

fn table_columns(source: &str) -> BTreeMap<String, Vec<String>> {
    let table =
        Regex::new(r"(?s)CREATE TABLE(?: IF NOT EXISTS)? `([a-z0-9_]+)` \((.*?)\n\s*\)\s*ENGINE=")
            .unwrap();
    let column = Regex::new(r"(?m)^\s*`([a-z0-9_]+)`\s+[A-Za-z]+").unwrap();
    table
        .captures_iter(source)
        .map(|part| {
            (
                part[1].to_owned(),
                column
                    .captures_iter(&part[2])
                    .map(|capture| capture[1].to_owned())
                    .collect(),
            )
        })
        .collect()
}

pub fn sql_literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Bool(value) => if *value { "1" } else { "0" }.into(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => quote(value),
        Value::Array(_) | Value::Object(_) => quote(&serde_json::to_string(value).unwrap()),
    }
}

fn quote(value: &str) -> String {
    format!(
        "'{}'",
        value
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}

pub fn sha(content: &[u8]) -> String {
    hex::encode(Sha256::digest(content))
}

pub fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            } else {
                result.insert(path.clone(), fs::read(path).unwrap());
            }
        }
    }
    result
}
