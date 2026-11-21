use std::io::Write;

use super::*;
use crate::recovery::clone::native::{json as document, lifecycle::recover};

const CONFLICT: &[u8] = b"{\"status\":\"controller_recovered\"";
const COMPLETED: &str = "控制恢复结果已存在且与原意图一致";

#[test]
fn finalizes_the_same_result_after_an_atomic_publish_conflict() {
    for use_durable in [false, true] {
        let fixture = Fixture::new();
        let (owner, original_owner, owner_binding) = recorded_owner(&fixture, use_durable);
        let lock = fixture.run.join("run.lock");
        let replacement_lock = fixture.local.join("replacement-lock");
        // 与原锁同时创建，保证两者具有不同的物理目录身份。
        fs::create_dir(&replacement_lock).unwrap();
        let command = CloneCommand::Recover {
            run_dir: fixture.run.clone(),
            owner_binding: owner_binding.clone(),
        };
        let mut first_result = None;
        let failure = recover::run_with_result_publisher(
            &fixture.backend,
            &fixture.run,
            &owner_binding,
            |path, value| {
                assert!(!lock.exists());
                assert_interrupted(&fixture);
                let mut occupied = fs::File::create_new(path)?;
                occupied.write_all(CONFLICT)?;
                occupied.sync_all()?;
                drop(occupied);
                first_result = Some((path.to_path_buf(), value.clone()));
                document::publish_new(path, value, "控制恢复结果")
            },
        )
        .unwrap_err()
        .to_string();
        assert!(failure.contains("控制恢复结果"), "{failure}");
        let (result_path, expected) = first_result.unwrap();
        let intent = assert_result_paths(&fixture, &result_path);
        assert_eq!(fs::read(&result_path).unwrap(), CONFLICT);
        let reason = assert_rejected_without_changes(&fixture, &command);
        assert!(!reason.contains(COMPLETED), "{reason}");
        assert!(!lock.exists());

        // 只撤销本次测试注入的占位文件，保留原恢复意图和状态。
        assert_eq!(fs::read(&result_path).unwrap(), CONFLICT);
        fs::remove_file(&result_path).unwrap();
        assert_finalization_rejects_drift(
            &fixture,
            &command,
            &intent,
            &replacement_lock,
            &original_owner,
        );
        let mut original = snapshot(&fixture.run);
        let result = native::lifecycle::run(&command, &fixture.backend).unwrap();
        assert_eq!(result, expected);
        assert_eq!(read_json(&result_path), result);
        assert_result(&fixture, &owner, &result);
        let raw_result = fs::read(&result_path).unwrap();
        original.insert(result_path, raw_result);
        assert_eq!(snapshot(&fixture.run), original);
        let reason = assert_rejected_without_changes(&fixture, &command);
        assert!(reason.contains(COMPLETED), "{reason}");
        assert!(!lock.exists());
    }
}

#[test]
fn rejects_another_publication_after_the_complete_result_was_written() {
    for use_durable in [false, true] {
        let fixture = Fixture::new();
        let (owner, _, owner_binding) = recorded_owner(&fixture, use_durable);
        let command = CloneCommand::Recover {
            run_dir: fixture.run.clone(),
            owner_binding: owner_binding.clone(),
        };
        let mut published = None;
        let failure = recover::run_with_result_publisher(
            &fixture.backend,
            &fixture.run,
            &owner_binding,
            |path, value| {
                assert!(!fixture.run.join("run.lock").exists());
                assert_interrupted(&fixture);
                document::publish_new(path, value, "控制恢复结果")?;
                published = Some((path.to_path_buf(), value.clone()));
                Err("测试：控制恢复结果发布后失响".into())
            },
        )
        .unwrap_err()
        .to_string();
        assert_eq!(failure, "测试：控制恢复结果发布后失响");
        let (result_path, expected) = published.unwrap();
        assert_result_paths(&fixture, &result_path);
        let result = read_json(&result_path);
        assert_eq!(result, expected);
        assert_result(&fixture, &owner, &result);
        let reason = assert_rejected_without_changes(&fixture, &command);
        assert!(reason.contains(COMPLETED), "{reason}");

        // JSON 仍可解析时，缺少原子发布的末尾 LF 也意味着收据字节不完整。
        let raw = fs::read(&result_path).unwrap();
        let without_lf = raw.strip_suffix(b"\n").unwrap();
        fs::write(&result_path, without_lf).unwrap();
        assert_eq!(read_json(&result_path), result);
        let reason = assert_rejected_without_changes(&fixture, &command);
        assert!(!reason.contains(COMPLETED), "{reason}");
        assert!(!fixture.run.join("run.lock").exists());
    }
}

fn recorded_owner(fixture: &Fixture, use_durable: bool) -> (Value, Vec<u8>, PathBuf) {
    let lock_owner = fixture.run.join("run.lock/owner.json");
    let (owner, raw, controller) = prepare_dead_owner(fixture, &lock_owner);
    let selected = if use_durable { controller } else { lock_owner };
    (owner, raw, selected)
}

fn assert_interrupted(fixture: &Fixture) {
    let saved = fixture.state();
    let attempts = saved["attempts"].as_array().unwrap();
    let [attempt] = attempts.as_slice() else {
        panic!("恢复必须保持唯一原阶段");
    };
    assert_eq!(attempt["number"], 1);
    assert_eq!(attempt["status"], "failed");
    assert_eq!(attempt["error_type"], "ControllerInterrupted");
    assert!(attempt["result"].is_null());
}

fn assert_result_paths(fixture: &Fixture, path: &Path) -> PathBuf {
    assert_eq!(path.parent(), Some(fixture.run.as_path()));
    let id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.strip_prefix("recovered-"))
        .unwrap();
    assert_eq!(id, uuid::Uuid::parse_str(id).unwrap().simple().to_string());
    let intent = fixture.run.join(format!("recovery-{id}.intent.json"));
    assert_eq!(
        receipts(&fixture.run, "recovery-", ".intent.json").as_slice(),
        std::slice::from_ref(&intent)
    );
    let [result]: [PathBuf; 1] = receipts(&fixture.run, "recovered-", ".json")
        .try_into()
        .unwrap();
    assert_eq!(result.as_path(), path);
    assert_eq!(read_json(&intent)["status"], "controller_recovery_started");
    intent
}

fn assert_result(fixture: &Fixture, owner: &Value, result: &Value) {
    assert_eq!(result["status"], "controller_recovered");
    assert_eq!(result["owner"], *owner);
    assert_eq!(result["remote_writes"], 0);
    assert_eq!(result["copy_requires_reconciliation"], true);
    let state_digest = hex::encode(Sha256::digest(
        serde_json::to_vec(&fixture.state()).unwrap(),
    ));
    assert_eq!(result["state_sha256"], state_digest);
    assert_interrupted(fixture);
}

fn assert_rejected_without_changes(fixture: &Fixture, command: &CloneCommand) -> String {
    let before = snapshot(&fixture.run);
    let error = native::lifecycle::run(command, &fixture.backend)
        .unwrap_err()
        .to_string();
    assert_eq!(snapshot(&fixture.run), before);
    error
}

fn assert_finalization_rejects_drift(
    fixture: &Fixture,
    command: &CloneCommand,
    intent: &Path,
    replacement_lock: &Path,
    owner_bytes: &[u8],
) {
    let state_path = fixture.run.join("state.json");
    let state = fs::read(&state_path).unwrap();
    let mut changed = fixture.state();
    changed["attempts"][0]["sources"]["fixture"] = json!(false);
    write_json(&state_path, &changed);
    assert_rejected_without_changes(fixture, command);
    fs::write(&state_path, state).unwrap();

    // 同一语义的 owner 出现在新物理目录时，也不能接管该锁。
    let lock = fixture.run.join("run.lock");
    let owner = lock.join("owner.json");
    fs::rename(replacement_lock, &lock).unwrap();
    fs::write(&owner, owner_bytes).unwrap();
    assert_rejected_without_changes(fixture, command);
    fs::remove_file(owner).unwrap();
    fs::remove_dir(lock).unwrap();

    let duplicate = fixture.run.join(format!(
        "recovery-{}.intent.json",
        uuid::Uuid::new_v4().simple()
    ));
    fs::copy(intent, &duplicate).unwrap();
    assert_rejected_without_changes(fixture, command);
    fs::remove_file(duplicate).unwrap();
}
