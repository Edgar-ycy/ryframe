use super::*;
use std::path::PathBuf;

struct Fixture {
    _temporary: tempfile::TempDir,
    backend: PathBuf,
    target: PathBuf,
    inventory: PathBuf,
    payload: Value,
    receipt: Value,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let base = backend.join(".local-tests/inventory-history-unit");
        fs::create_dir_all(&base).unwrap();
        let temporary = tempfile::tempdir_in(base).unwrap();
        let target = temporary.path().join("target");
        let inventory = target.join("inventory-initial");
        fs::create_dir_all(&inventory).unwrap();
        let scope = "inventory-scope";
        let mut observations = serde_json::Map::new();
        let mut images = serde_json::Map::new();
        let databases = KEYS
            .iter()
            .enumerate()
            .map(|(index, key)| {
                let name = format!("database_{index}");
                let image = json!({"resource":{"kind":"database","scope_id":scope,
                "server_uuid":"server-identity","database":name},
                "row_count":0,"all_tables":[],"ownership":[]});
                observations.insert((*key).into(), image.clone());
                images.insert(name.clone(), image);
                json!({"key":key,"database":name,"server_uuid":"server-identity"})
            })
            .collect::<Vec<_>>();
        let binding = json!({"scope_id":scope,"普通来源":"绑定"});
        for name in ["binding-before.json", "binding-after.json"] {
            json_io::write_new(&inventory.join(name), &binding).unwrap();
        }
        let mut raw = serde_json::Map::new();
        for phase in ["before", "after"] {
            for key in KEYS {
                let name = format!("{phase}-target-{key}.json");
                let path = inventory.join(&name);
                fs::write(&path, b"raw maintenance CLI output\n").unwrap();
                let actual = digest::file(&path).unwrap();
                raw.insert(name, json!({"bytes":actual.bytes,"sha256":actual.sha256}));
            }
        }
        let receipt = json!({"format_version":1,"status":"side_inventory_captured",
            "side":"target","scope_id":scope,"keys":KEYS,"observations":observations,
            "inventories":raw,"binding_sha256":digest::plan_hash(&binding).unwrap(),
            "remote_writes":0,"producer_stopped_proven":false,"fresh_target_proven":false,
            "target_ready":false,"clone_verified":false,"restore_qualified":false});
        let payload = json!({"format_version":1,"operation":"inventory-history",
            "backend":backend,"target":target,"request":{"target":{"scope_id":scope,
            "databases":databases}},"initial":{"receipt":null,"observations":images},
            "resumed":false,"inventory_directory":null});
        let mut fixture = Self {
            _temporary: temporary,
            backend,
            target,
            inventory,
            payload,
            receipt,
        };
        fixture.save();
        fixture
    }

    fn save(&mut self) {
        let path = self.inventory.join("inventory.json");
        fs::write(&path, serde_json::to_vec(&self.receipt).unwrap()).unwrap();
        self.payload["initial"]["receipt"] = common::binding(&path).unwrap();
    }

    fn call(&self) -> Result<Value> {
        super::super::execute(&serde_json::to_vec(&self.payload)?)
    }

    fn move_inventory(&mut self, path: PathBuf) {
        fs::rename(&self.inventory, &path).unwrap();
        self.inventory = path;
        self.save();
    }
}

#[test]
fn complete_success_is_read_only_and_does_not_claim_target_authority() {
    let fixture = Fixture::new();
    let before = tree::snapshot(&fixture.backend, &fixture.target, false).unwrap();
    assert_eq!(
        fixture.call().unwrap(),
        json!({"status":"inventory_history_verified","remote_writes":0})
    );
    assert_eq!(
        before,
        tree::snapshot(&fixture.backend, &fixture.target, false).unwrap()
    );
}

#[test]
fn reference_explicit_directory_and_resumed_generation_keep_distinct_rules() {
    let mut explicit = Fixture::new();
    let destination = explicit._temporary.path().join("reference-inventory");
    explicit.move_inventory(destination);
    explicit.payload["inventory_directory"] = json!(explicit.inventory);
    explicit.call().unwrap();
    let mut resumed = Fixture::new();
    resumed.move_inventory(resumed.target.join("inventory-resume-r001"));
    assert!(resumed.call().is_err());
    resumed.payload["resumed"] = json!(true);
    fs::write(
        resumed.inventory.join("failure.json"),
        b"resolved externally",
    )
    .unwrap();
    resumed.call().unwrap();
    let sibling = resumed._temporary.path().join("inventory-resume-other");
    resumed.move_inventory(sibling);
    resumed.payload["inventory_directory"] = json!(resumed.inventory);
    assert!(resumed.call().is_err());
}

#[test]
fn receipt_side_scope_status_version_keys_and_observation_set_are_exact() {
    for (field, replacement) in [
        ("side", json!("source")),
        ("scope_id", json!("other-scope")),
        ("status", json!("completed")),
        ("format_version", json!(true)),
        (
            "keys",
            json!(["shared", "shared-control", "dedicated-a", "dedicated-b"]),
        ),
    ] {
        let mut fixture = Fixture::new();
        fixture.receipt[field] = replacement;
        fixture.save();
        assert!(fixture.call().is_err(), "{field}");
    }
    for remove in [true, false] {
        let mut fixture = Fixture::new();
        let observations = fixture.receipt["observations"].as_object_mut().unwrap();
        if remove {
            observations.remove(KEYS[0]);
        } else {
            observations.insert("extra".into(), json!({}));
        }
        fixture.save();
        assert!(fixture.call().is_err());
    }
}

#[test]
fn all_eight_outputs_and_their_exact_raw_digests_are_required() {
    for mode in 0..4 {
        let mut fixture = Fixture::new();
        let name = "before-target-shared.json";
        match mode {
            0 => {
                fs::remove_file(fixture.inventory.join(name)).unwrap();
            }
            1 => {
                fixture.receipt["inventories"]
                    .as_object_mut()
                    .unwrap()
                    .remove(name);
            }
            2 => {
                fixture.receipt["inventories"]["extra.json"] =
                    json!({"bytes":0,"sha256":"0".repeat(64)});
            }
            _ => {
                fs::write(fixture.inventory.join(name), b"different raw output").unwrap();
            }
        }
        fixture.save();
        assert!(fixture.call().is_err(), "mode={mode}");
    }
}

#[test]
fn both_binding_images_and_initial_receipt_binding_are_verified() {
    for name in ["binding-before.json", "binding-after.json"] {
        let fixture = Fixture::new();
        fs::write(fixture.inventory.join(name), b"{}").unwrap();
        assert!(fixture.call().is_err());
    }
    let mut fixture = Fixture::new();
    fixture.payload["initial"]["receipt"]["sha256"] = json!("0".repeat(64));
    assert!(fixture.call().is_err());
}

#[test]
fn four_request_keys_physical_mapping_and_initial_images_cannot_be_substituted() {
    for mode in 0..6 {
        let mut fixture = Fixture::new();
        match mode {
            0 => {
                fixture.payload["request"]["target"]["databases"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            1 => fixture.payload["request"]["target"]["databases"][1]["key"] = json!(KEYS[0]),
            2 => {
                fixture.payload["request"]["target"]["databases"][0]["server_uuid"] =
                    json!("other-server")
            }
            3 => {
                fixture.payload["request"]["target"]["databases"][0]["database"] =
                    json!("other-database")
            }
            4 => fixture.payload["initial"]["observations"]["database_0"]["row_count"] = json!(1),
            _ => {
                fixture.receipt["observations"][KEYS[0]]["resource"]["scope_id"] =
                    json!("other-scope")
            }
        }
        fixture.save();
        assert!(fixture.call().is_err(), "mode={mode}");
    }
}

#[test]
fn locks_unresolved_failure_and_links_never_grant_resume_qualification() {
    let mut fixture = Fixture::new();
    fs::create_dir(fixture.inventory.join("initialize.lock")).unwrap();
    assert!(fixture.call().is_err());
    fs::remove_dir(fixture.inventory.join("initialize.lock")).unwrap();
    fs::write(fixture.inventory.join("failure.json"), b"unresolved").unwrap();
    assert!(fixture.call().is_err());
    fixture.payload["resumed"] = json!(true);
    fixture.call().unwrap();
    #[cfg(any(unix, windows))]
    {
        fs::remove_file(fixture.inventory.join("failure.json")).unwrap();
        let link = fixture.inventory.join("failure.json");
        #[cfg(unix)]
        std::os::unix::fs::symlink(fixture.inventory.join("missing"), &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(fixture.inventory.join("missing"), &link).unwrap();
        assert!(fixture.call().is_err());
    }
}

#[test]
fn final_guard_rejects_tree_drift_and_rechecks_all_bound_files() {
    let fixture = Fixture::new();
    let result = verify_with(&fixture.backend, &fixture.target, &fixture.payload, || {
        fs::write(fixture.inventory.join("unexpected.json"), b"new evidence").unwrap();
    });
    assert!(result.is_err());
}

#[cfg(any(unix, windows))]
#[test]
fn raw_binding_and_inventory_parent_links_are_rejected() {
    for name in ["before-target-shared.json", "binding-before.json", "parent"] {
        let fixture = Fixture::new();
        if name == "parent" {
            let original = fixture.target.join("original-inventory");
            fs::rename(&fixture.inventory, &original).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(original, &fixture.inventory).unwrap();
            #[cfg(windows)]
            std::os::windows::fs::symlink_dir(original, &fixture.inventory).unwrap();
        } else {
            let original = fixture.inventory.join("original-output");
            fs::rename(fixture.inventory.join(name), &original).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(original, fixture.inventory.join(name)).unwrap();
            #[cfg(windows)]
            std::os::windows::fs::symlink_file(original, fixture.inventory.join(name)).unwrap();
        }
        assert!(fixture.call().is_err(), "{name}");
    }
}

#[test]
fn held_inputs_block_windows_writes_or_detect_unix_content_drift() {
    let fixture = Fixture::new();
    let mut changed = false;
    let result = verify_with(&fixture.backend, &fixture.target, &fixture.payload, || {
        changed = fs::write(
            fixture.inventory.join("after-target-shared.json"),
            b"changed",
        )
        .is_ok();
    });
    #[cfg(windows)]
    {
        assert!(!changed);
        assert!(result.is_ok());
    }
    #[cfg(unix)]
    {
        assert!(changed);
        assert!(result.is_err());
    }
}

#[test]
fn operation_has_strict_fields_types_and_safe_diagnostics() {
    let fixture = Fixture::new();
    let encoded = serde_json::to_string(&fixture.payload).unwrap();
    let duplicate = format!("{{\"resumed\":false,{}", &encoded[1..]);
    assert!(super::super::execute(duplicate.as_bytes()).is_err());
    for field in ["initial", "request", "resumed", "inventory_directory"] {
        let mut value = fixture.payload.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(super::super::execute(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    for (field, replacement) in [
        ("extra", json!("PRIVATE-SENTINEL")),
        ("resumed", json!(1)),
        ("inventory_directory", json!(1)),
    ] {
        let mut value = fixture.payload.clone();
        value[field] = replacement;
        let error = super::super::execute(&serde_json::to_vec(&value).unwrap()).unwrap_err();
        assert!(!super::super::safe_message(error.as_ref()).contains("PRIVATE-SENTINEL"));
    }
}
