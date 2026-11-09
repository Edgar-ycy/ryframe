use super::*;

const SCOPE: &str = "fixture-source";

fn selected_item() -> Value {
    json!({"key":"shared","source_database":"source_shared","target_database":"target_shared",
        "tables":{"tenant_table":17},
        "artifact":{"file":"databases/shared.sql","bytes":17,"sha256":"a".repeat(64)},
        "preserve_target_ownership":true,
    })
}

#[test]
fn exact_database_step_rejects_changed_id_artifact_item_or_kind() {
    let item = selected_item();
    let exported = json!({"key":"shared","tables":item["tables"],"artifact":item["artifact"]});
    let declared = json!({"key":"shared"});
    let kept = BTreeSet::from(["ryframe_resource_ownership".into()]);
    let database = Database {
        id: "database-exact".into(),
        item: &item,
        exported: &exported,
        source: DatabaseDeclaration {
            value: &declared,
            kept: kept.clone(),
        },
        target: DatabaseDeclaration {
            value: &declared,
            kept,
        },
    };
    let sha = item["artifact"]["sha256"].as_str().unwrap();
    let mut step = Step {
        id: database.id.clone(),
        item: &item,
        kind: Kind::Database,
        artifact: sha,
    };
    assert!(database.matches(&step));
    step.id.push_str("-other");
    assert!(!database.matches(&step));
    step.id = database.id.clone();
    step.kind = Kind::Object;
    assert!(!database.matches(&step));
    step.kind = Kind::Database;
    step.artifact = "wrong";
    assert!(!database.matches(&step));
    let mut changed = item.clone();
    changed["artifact"]["file"] = json!("databases/foreign.sql");
    step.artifact = sha;
    step.item = &changed;
    assert!(!database.matches(&step));
}

#[test]
fn only_the_complete_fixed_database_vector_can_be_indexed() {
    let rows = json!(KEYS.map(|(key, kind, mode)| json!({"key":key,"kind":kind,"mode":mode})));
    assert_eq!(index(&rows).unwrap().len(), 4);
    let mut changed = rows.clone();
    changed[0]["key"] = changed[1]["key"].clone();
    assert!(index(&changed).is_err());
    changed = rows.clone();
    changed[0]["key"] = json!("foreign-key");
    assert!(index(&changed).is_err());
    changed = rows;
    changed.as_array_mut().unwrap().pop();
    assert!(index(&changed).is_err());
}

struct InventoryFixture {
    backend: tempfile::TempDir,
    work: PathBuf,
    directory: PathBuf,
    observed: Observed,
}

impl InventoryFixture {
    fn new() -> Self {
        let backend = tempfile::tempdir_in(crate::workspace::root_dir()).unwrap();
        let work = backend.path().join(".local-tests/copy");
        let directory = work.join("source-inventory");
        fs::create_dir_all(&directory).unwrap();
        let observations: BTreeMap<String, Value> = KEYS
            .into_iter()
            .map(|(key, _, _)| {
                (
                    key.to_owned(),
                    json!({"resource":{"database":key},"tables":{}}),
                )
            })
            .collect();
        let mut evidence_files = Vec::new();
        for phase in ["before", "after"] {
            let name = format!("binding-{phase}.json");
            evidence_files.push(write_file(&directory, &name, &json!({"bound":true})));
            for (key, _, _) in KEYS {
                let name = format!("{phase}-target-{key}.json");
                evidence_files.push(write_file(&directory, &name, &json!({"key":key})));
            }
        }
        evidence_files.push(write_file(
            &directory,
            "inventory.json",
            &json!({
                "status":"side_inventory_captured","side":"source","scope_id":SCOPE,
                "remote_writes":0,"observations":observations,
            }),
        ));
        let receipt = common::binding(&directory.join("inventory.json")).unwrap();
        Self {
            backend,
            work,
            directory,
            observed: Observed {
                observations,
                receipt,
                evidence_files,
            },
        }
    }

    fn seal(&self) -> Result<Inventory> {
        Inventory::seal(
            self.backend.path(),
            &self.work,
            &self.directory,
            CloneSide::Source,
            SCOPE,
            &self.observed,
        )
    }
}

fn write_file(root: &Path, name: &str, value: &Value) -> Value {
    let path = root.join(name);
    document::write_new(&path, value).unwrap();
    let digest = digest::file(&path).unwrap();
    json!({"path":name,"type":"file","bytes":digest.bytes,"sha256":digest.sha256})
}

#[test]
fn evidence_rechecks_only_the_original_two_rounds_and_bound_receipt() {
    let fixture = InventoryFixture::new();
    let seal = fixture.seal().unwrap();
    seal.check(fixture.backend.path()).unwrap();
    document::write_new(
        &fixture.directory.join("failure.json"),
        &json!({"status":"failed"}),
    )
    .unwrap();
    assert!(seal.check(fixture.backend.path()).is_err());

    let mut fixture = InventoryFixture::new();
    fixture
        .observed
        .evidence_files
        .retain(|file| file["path"] != "after-target-shared.json");
    assert!(fixture.seal().is_err());
    let mut fixture = InventoryFixture::new();
    fixture.observed.evidence_files[2]["sha256"] = json!("0".repeat(64));
    assert!(fixture.seal().is_err());
    let mut fixture = InventoryFixture::new();
    fixture.observed.observations.get_mut("shared").unwrap()["tables"] = json!({"injected":true});
    assert!(fixture.seal().is_err());
}
