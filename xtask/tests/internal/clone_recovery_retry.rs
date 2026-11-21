#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

use super::*;
use crate::recovery::clone::native::lifecycle::state;

#[path = "clone_recovery_result_finalization.rs"]
mod result_finalization;

#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x0000_0001;
#[cfg(windows)]
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

#[cfg(windows)]
#[test]
fn recovers_the_same_controller_after_windows_denies_lock_directory_removal() {
    for use_durable in [false, true] {
        let fixture = Fixture::new();
        let lock = fixture.run.join("run.lock");
        let owner_path = lock.join("owner.json");
        let (owner, original_owner, controller) = prepare_dead_owner(&fixture, &owner_path);
        let command = CloneCommand::Recover {
            run_dir: fixture.run.clone(),
            owner_binding: if use_durable {
                controller
            } else {
                owner_path.clone()
            },
        };
        let recover = || native::lifecycle::run(&command, &fixture.backend);
        let (uncommitted, uncommitted_bytes) =
            retain_uncommitted_intent(&fixture, &owner_path, &original_owner, &recover);

        // 保留读取与写入共享，只阻止删除控制锁目录。
        let directory = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&lock)
            .unwrap();
        let error = recover().unwrap_err().to_string();
        assert!(error.contains("控制锁目录清理失败"), "{error}");
        assert_eq!(fs::read(&owner_path).unwrap(), original_owner);
        let interrupted = fixture.state();
        assert_eq!(interrupted["attempts"].as_array().unwrap().len(), 1);
        assert_eq!(interrupted["attempts"][0]["status"], "failed");
        assert_eq!(
            interrupted["attempts"][0]["error_type"],
            "ControllerInterrupted"
        );
        assert!(interrupted["attempts"][0]["result"].is_null());
        let intents = receipts(&fixture.run, "recovery-", ".intent.json");
        assert_eq!(intents.len(), 2);
        assert!(intents.contains(&uncommitted));
        assert_eq!(fs::read(&uncommitted).unwrap(), uncommitted_bytes);
        assert!(receipts(&fixture.run, "recovered-", ".json").is_empty());
        let intent = intents
            .iter()
            .find(|path| path.as_path() != uncommitted.as_path())
            .unwrap();
        let id = intent
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .strip_prefix("recovery-")
            .unwrap()
            .strip_suffix(".intent.json")
            .unwrap();
        assert_eq!(id, uuid::Uuid::parse_str(id).unwrap().simple().to_string());
        assert_eq!(read_json(intent)["status"], "controller_recovery_started");
        drop(directory);

        let mut original_run = snapshot(&fixture.run);
        let changed_owner = serde_json::to_vec(&owner).unwrap();
        assert_ne!(changed_owner, original_owner);
        fs::write(&owner_path, changed_owner).unwrap();
        assert_eq!(read_json(&owner_path), owner);
        let changed_run = snapshot(&fixture.run);
        assert!(recover().is_err());
        assert_eq!(snapshot(&fixture.run), changed_run);
        fs::write(&owner_path, &original_owner).unwrap();
        assert_eq!(snapshot(&fixture.run), original_run);

        let result = recover().unwrap();
        assert_eq!(result["status"], "controller_recovered");
        assert_eq!(result["owner"], owner);
        assert_eq!(result["remote_writes"], 0);
        assert_eq!(result["copy_requires_reconciliation"], true);
        assert!(!lock.exists());
        let result_path = fixture.run.join(format!("recovered-{id}.json"));
        assert_eq!(
            receipts(&fixture.run, "recovered-", ".json").as_slice(),
            std::slice::from_ref(&result_path)
        );
        assert_eq!(read_json(&result_path), result);
        original_run.remove(&owner_path).unwrap();
        original_run.insert(result_path.clone(), fs::read(&result_path).unwrap());
        assert_eq!(snapshot(&fixture.run), original_run);
        assert_eq!(fs::read(&uncommitted).unwrap(), uncommitted_bytes);
        assert!(recover().is_err());
        assert_eq!(snapshot(&fixture.run), original_run);
    }
}

#[cfg(windows)]
fn retain_uncommitted_intent(
    fixture: &Fixture,
    owner_path: &Path,
    original_owner: &[u8],
    recover: &dyn Fn() -> crate::Result<Value>,
) -> (PathBuf, Vec<u8>) {
    let path = fixture.run.join("state.json");
    let original = fs::read(&path).unwrap();
    // 文件允许读取与写入共享，但禁止原子替换所需的删除共享。
    let state_file = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&path)
        .unwrap();
    assert!(receipts(&fixture.run, "recovery-", ".intent.json").is_empty());
    let error = recover().unwrap_err().to_string();
    assert!(
        error.contains("无法发布复制状态") && error.contains("state.json"),
        "{error}"
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::read(owner_path).unwrap().as_slice(), original_owner);
    assert!(receipts(&fixture.run, "recovered-", ".json").is_empty());
    let [intent]: [PathBuf; 1] = receipts(&fixture.run, "recovery-", ".intent.json")
        .try_into()
        .unwrap();
    assert_eq!(read_json(&intent)["status"], "controller_recovery_started");
    let bytes = fs::read(&intent).unwrap();
    drop(state_file);
    (intent, bytes)
}

fn prepare_dead_owner(fixture: &Fixture, owner_path: &Path) -> (Value, Vec<u8>, PathBuf) {
    fixture.init().unwrap();
    let (number, attempt) =
        state::begin(&fixture.run, "copy", "resume", &json!({"fixture": true})).unwrap();
    fs::create_dir(owner_path.parent().unwrap()).unwrap();
    let owner = json!({
        "format_version":1,
        "identity":{"pid":u32::MAX,"started":"dead-fixture","executable":"dead-fixture"},
        "directory":fixture.run,
        "manifest_sha256":binding(&fixture.run.join("manifest.json"))["sha256"],
    });
    let original = format!(" \n{}\n\t", serde_json::to_string(&owner).unwrap()).into_bytes();
    fs::write(owner_path, &original).unwrap();
    state::bind_controller(&fixture.run, number, &owner, &attempt).unwrap();
    (
        owner,
        original,
        state::controller_path(&fixture.run, number),
    )
}

fn receipts(run: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    fs::read_dir(run)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_str().unwrap();
            name.starts_with(prefix) && name.ends_with(suffix)
        })
        .map(|entry| entry.path())
        .collect()
}
