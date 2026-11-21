use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{Fake, Fixture, KEYS, SCOPE, bound, database, digest, json_io};
use crate::clone_native_tests::snapshot;

const CHECKPOINT_ERROR: &str = "库存发布后的检查点失败";
const UNKNOWN_BYTES: &[u8] = b"{\"external\":\"unknown publication evidence\"}\n";

struct OriginalPublication {
    receipt: Value,
    materials: BTreeMap<PathBuf, Value>,
    files: BTreeMap<PathBuf, Vec<u8>>,
}

#[test]
fn checkpoint_error_is_returned_and_no_inventory_is_published() {
    let fixture = Fixture::new();
    let fake = Fake::new(&fixture, "none");
    let calls = Cell::new(0);
    let result = database::capture_with(&fixture.context, &fake, &|| {
        calls.set(calls.get() + 1);
        if calls.get() == 11 {
            Err("explicit checkpoint failure".into())
        } else {
            Ok(())
        }
    });
    let error = result.err().unwrap();
    assert!(error.to_string().contains("CheckpointError"));
    assert!(
        error
            .source()
            .unwrap()
            .to_string()
            .contains("explicit checkpoint failure")
    );
    assert!(
        !fixture
            .context
            .observation_dir
            .join("inventory/inventory.json")
            .exists()
    );
}

#[test]
fn checkpoint_error_after_publish_preserves_unknown_evidence() {
    for injected in ["unknown-post-publication.json", "inventory.json"] {
        check_failed_publication(injected);
    }
}

fn check_failed_publication(injected_name: &str) {
    let fixture = Fixture::new();
    let transport = Fake::new(&fixture, "none");
    let directory = fixture.context.observation_dir.join("inventory");
    let inventory = directory.join("inventory.json");
    let injected = directory.join(injected_name);
    let original = RefCell::new(None);
    let result = database::capture_with(&fixture.context, &transport, &|| {
        if !inventory.try_exists()? {
            return Ok(());
        }
        // 由真实发布文件触发注入，待两轮原件和绑定全部写完。
        assert!(original.borrow().is_none(), "发布后只执行一次故障注入");
        *original.borrow_mut() = Some(read_publication(&directory, &inventory));
        if injected == inventory {
            fs::remove_file(&inventory)?;
        }
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&injected)?
            .write_all(UNKNOWN_BYTES)?;
        Err(CHECKPOINT_ERROR.into())
    });
    assert_eq!(
        result.err().expect("发布后检查点必须失败").to_string(),
        CHECKPOINT_ERROR
    );
    assert_eq!(transport.inventories.get(), 8);
    assert_eq!(transport.identities.get(), 16);
    assert_eq!(transport.owners.get(), 24);
    assert_eq!(*transport.order.borrow(), KEYS.repeat(2));

    let OriginalPublication {
        receipt,
        materials,
        mut files,
    } = original.into_inner().expect("必须观察到真实发布");
    let mut observed = snapshot(&directory);
    let failure = observed
        .remove(&directory.join("failure.json"))
        .expect("必须保留真实失败收据");
    assert_eq!(
        serde_json::from_slice::<Value>(&failure).unwrap(),
        json!({"format_version": 1, "status": "side_inventory_failed", "stage": "publish",
            "error_type": "RustError", "remote_writes": 0, "target_ready": false, "clone_verified": false})
    );
    for (path, descriptor) in materials {
        assert_eq!(bound(&path), descriptor, "原始材料变化：{path:?}");
    }
    assert!(files.remove(&inventory).is_some());
    files.insert(injected.clone(), UNKNOWN_BYTES.to_vec());
    assert_eq!(observed, files, "发布失败后目录成员或原始字节变化");
    if injected == inventory {
        assert_ne!(bound(&inventory), receipt, "保留的必须是未知替换文件");
    } else {
        assert!(!inventory.try_exists().unwrap(), "应撤回本次精确发布");
    }
}

fn read_publication(directory: &Path, inventory: &Path) -> OriginalPublication {
    let receipt = json_io::read(inventory).unwrap();
    assert_eq!(receipt["status"], "side_inventory_captured");
    assert_eq!(receipt["side"], "target");
    assert_eq!(receipt["scope_id"], SCOPE);
    assert_eq!(receipt["keys"], json!(KEYS));
    assert_eq!(receipt["remote_writes"], 0);
    assert_eq!(receipt["inventories"].as_object().unwrap().len(), 8);

    let before_binding = directory.join("binding-before.json");
    let after_binding = directory.join("binding-after.json");
    assert_eq!(
        fs::read(&before_binding).unwrap(),
        fs::read(&after_binding).unwrap()
    );
    assert_eq!(
        receipt["binding_sha256"],
        digest::plan_hash(&json_io::read(&before_binding).unwrap()).unwrap()
    );
    let mut materials = BTreeMap::from([
        (before_binding.clone(), bound(&before_binding)),
        (after_binding.clone(), bound(&after_binding)),
    ]);
    for key in KEYS {
        let before_raw = directory.join(format!("before-target-{key}.json"));
        let after_raw = directory.join(format!("after-target-{key}.json"));
        assert_eq!(fs::read(before_raw).unwrap(), fs::read(after_raw).unwrap());
        for phase in ["before", "after"] {
            let name = format!("{phase}-target-{key}.json");
            let path = directory.join(&name);
            let descriptor = bound(&path);
            assert_eq!(
                receipt["inventories"][&name],
                json!({"bytes": descriptor["bytes"], "sha256": descriptor["sha256"]}),
                "发布收据中的库存摘要必须绑定原件：{name}"
            );
            assert!(materials.insert(path, descriptor).is_none());
        }
    }
    OriginalPublication {
        receipt: bound(inventory),
        materials,
        files: snapshot(directory),
    }
}
