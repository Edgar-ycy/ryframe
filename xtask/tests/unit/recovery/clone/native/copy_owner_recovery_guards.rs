use std::{cell::RefCell, collections::BTreeSet, ffi::OsString};

use super::*;
use crate::recovery::clone::native::{copy_recover, target_verify::guard::Generation};

const CONTENTION: &str = "控制或锁恢复正在执行，拒绝并发操作";

#[test]
fn reclaim_checks_both_copy_locks_before_releasing_either() {
    let (fixture, owner) = stopped_fixture();
    let target_record = fixture.add_lock(Role::Target, &owner);
    let ledger_record = fixture.add_lock(Role::Ledger, &owner);
    let target_lock = owner.lock_path(Role::Target);
    let ledger_lock = owner.lock_path(Role::Ledger);
    let ledger_receipt = ledger_lock.join("owner.json");
    let original_target = fs::read(target_lock.join("owner.json")).unwrap();
    let ledger_identity = directory_identity(ledger_lock);
    seed_ledger(&fixture);
    for role in [Role::Target, Role::Ledger] {
        let parent = owner.lock_path(role).parent().unwrap();
        drop(ProcessGuard::acquire_named(parent, role.guard_name()).unwrap());
    }
    let evidence = copy_evidence(&fixture);
    let originals = evidence.each_ref().map(|path| fs::read(path).unwrap());
    let directories = [
        fixture.copy.as_path(),
        fixture.target.as_path(),
        ledger_lock.parent().unwrap(),
    ];
    let members = directories.map(directory_members);
    assert_eq!(
        owner.verify_lock(Role::Target).unwrap(),
        Some(target_record.clone())
    );
    assert_eq!(
        owner.verify_lock(Role::Ledger).unwrap(),
        Some(ledger_record.clone())
    );

    fs::remove_file(&ledger_receipt).unwrap();
    let mut foreign = ledger_record;
    foreign["inode"] = json!(foreign["inode"].as_u64().unwrap() ^ 1);
    for (bad_receipt, reason) in [
        (
            None,
            "复制锁没有唯一的登记持有人，裸锁或其他会话锁必须核实现场",
        ),
        (
            Some(serde_json::to_vec(&foreign).unwrap()),
            "复制锁没有匹配的登记持有人或目录身份已变化",
        ),
    ] {
        if let Some(raw) = &bad_receipt {
            fs::write(&ledger_receipt, raw).unwrap();
        }
        let error = reclaim(&fixture, &owner, &|| Ok(())).unwrap_err();
        assert_eq!(error.to_string(), reason);
        assert_eq!(
            owner.verify_lock(Role::Target).unwrap(),
            Some(target_record.clone())
        );
        assert_eq!(
            directory_identity(target_lock),
            record_identity(&target_record)
        );
        assert_eq!(
            fs::read(target_lock.join("owner.json")).unwrap(),
            original_target
        );
        assert_eq!(directory_identity(ledger_lock), ledger_identity);
        if let Some(raw) = bad_receipt {
            assert_eq!(fs::read(&ledger_receipt).unwrap(), raw);
            assert_eq!(
                directory_members(ledger_lock),
                BTreeSet::from(["owner.json".into()])
            );
        } else {
            assert!(directory_members(ledger_lock).is_empty());
        }
        assert_eq!(directories.map(directory_members), members);
        assert_eq!(
            evidence.each_ref().map(|path| fs::read(path).unwrap()),
            originals
        );
        assert!(recovery_receipts(&fixture.copy).is_empty());
    }
}

#[test]
fn copy_reclaim_and_generation_share_the_native_target_guard() {
    let (fixture, historical) = stopped_fixture();
    let ledger_record = fixture.add_lock(Role::Ledger, &historical);
    let target_lock = historical.lock_path(Role::Target);
    let ledger_lock = historical.lock_path(Role::Ledger);
    let ledger_path = ledger_lock.join("owner.json");
    let original_ledger = fs::read(&ledger_path).unwrap();
    seed_ledger(&fixture);
    let evidence = copy_evidence(&fixture);
    let originals = evidence.each_ref().map(|path| fs::read(path).unwrap());
    let generation = Generation::acquire_copy(fixture.register().unwrap()).unwrap();
    let (live, _, live_record) = generation.copy_session().unwrap();
    let target_path = target_lock.join("owner.json");
    let original_target = fs::read(&target_path).unwrap();

    let error = reclaim(&fixture, &historical, &|| {
        panic!("已持有的 guard 应阻止进入回收窗口")
    })
    .unwrap_err();
    assert_eq!(error.to_string(), CONTENTION);
    assert_eq!(
        live.verify_lock(Role::Target).unwrap().as_ref(),
        Some(live_record)
    );
    assert_eq!(fs::read(&target_path).unwrap(), original_target);
    assert_eq!(fs::read(&ledger_path).unwrap(), original_ledger);
    assert_eq!(
        evidence.each_ref().map(|path| fs::read(path).unwrap()),
        originals
    );
    assert!(recovery_receipts(&fixture.copy).is_empty());
    generation.finish(Ok(())).unwrap();
    assert!(!target_lock.exists());

    let contender = RefCell::new(Some(fixture.register().unwrap()));
    let checkpoint = || {
        let Some(contender) = contender.borrow_mut().take() else {
            return Ok(());
        };
        assert!(!target_lock.exists());
        let Err(error) = Generation::acquire_copy(contender) else {
            panic!("已取得回收 guard 时不得建立目标锁");
        };
        assert_eq!(error.to_string(), CONTENTION);
        assert!(!target_lock.exists());
        assert_eq!(
            historical.verify_lock(Role::Ledger).unwrap(),
            Some(ledger_record.clone())
        );
        assert_eq!(fs::read(&ledger_path).unwrap(), original_ledger);
        assert!(recovery_receipts(&fixture.copy).is_empty());
        Ok(())
    };
    let result = reclaim(&fixture, &historical, &checkpoint).unwrap();
    assert!(contender.into_inner().is_none());
    assert_recovered(&fixture, &result, json!([ledger_record]));
    assert!(!target_lock.exists());
    assert!(!ledger_lock.exists());
    assert_eq!(
        evidence.each_ref().map(|path| fs::read(path).unwrap()),
        originals
    );
    assert_eq!(recovery_receipts(&fixture.copy).len(), 1);
}

#[cfg(windows)]
#[test]
fn reclaim_restores_owner_if_the_os_refuses_target_directory_removal() {
    use std::time::{Duration, UNIX_EPOCH};
    use winsafe::{self as w, co, prelude::*};

    let (fixture, owner) = stopped_fixture();
    let target_record = fixture.add_lock(Role::Target, &owner);
    let ledger_record = fixture.add_lock(Role::Ledger, &owner);
    let target = owner.lock_path(Role::Target);
    let ledger = owner.lock_path(Role::Ledger);
    let target_path = target.join("owner.json");
    let ledger_path = ledger.join("owner.json");
    let original_target = format!("  {}\n\n", serde_json::to_string(&target_record).unwrap());
    fs::write(&target_path, original_target.as_bytes()).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&target_path)
        .unwrap()
        .set_times(
            fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1_000_000_000)),
        )
        .unwrap();
    let original_modified = fs::metadata(&target_path).unwrap().modified().unwrap();
    let original_ledger = fs::read(&ledger_path).unwrap();
    seed_ledger(&fixture);
    let evidence = copy_evidence(&fixture);
    let originals = evidence.each_ref().map(|path| fs::read(path).unwrap());
    let (directory, _) = w::HFILE::CreateFile(
        &common::windows_path(target).unwrap(),
        co::GENERIC::READ,
        Some(co::FILE_SHARE::READ | co::FILE_SHARE::WRITE),
        None,
        co::DISPOSITION::OPEN_EXISTING,
        co::FILE_ATTRIBUTE::NORMAL,
        Some(co::FILE_FLAG::BACKUP_SEMANTICS | co::FILE_FLAG::OPEN_REPARSE_POINT),
        None,
        None,
    )
    .unwrap();

    let error = reclaim(&fixture, &owner, &|| Ok(())).unwrap_err();
    let io = error
        .downcast_ref::<std::io::Error>()
        .expect("目标目录删除应返回原始操作系统错误");
    assert!(matches!(io.raw_os_error(), Some(5 | 32)), "{io}");
    let prepared = recovery_receipts(&fixture.copy);
    assert_eq!(prepared.len(), 1);
    let prepared_bytes = fs::read(&prepared[0]).unwrap();
    let receipt = json_io::read(&prepared[0]).unwrap();
    assert_eq!(receipt["status"], "prepared");
    assert_eq!(receipt["locks"], json!([target_record, ledger_record]));
    assert_eq!(receipt["remote_writes"], 0);
    assert_eq!(directory_identity(target), record_identity(&target_record));
    assert_eq!(directory_identity(ledger), record_identity(&ledger_record));
    assert!(fs::metadata(&target_path).unwrap().modified().unwrap() > original_modified);
    assert_eq!(fs::read(&target_path).unwrap(), original_target.as_bytes());
    assert_eq!(fs::read(&ledger_path).unwrap(), original_ledger);
    assert_eq!(
        owner.verify_lock(Role::Target).unwrap(),
        Some(target_record.clone())
    );
    assert_eq!(
        owner.verify_lock(Role::Ledger).unwrap(),
        Some(ledger_record.clone())
    );
    assert_eq!(
        evidence.each_ref().map(|path| fs::read(path).unwrap()),
        originals
    );

    drop(directory);
    let result = reclaim(&fixture, &owner, &|| Ok(())).unwrap();
    assert_recovered(&fixture, &result, json!([target_record, ledger_record]));
    assert!(!target.exists());
    assert!(!ledger.exists());
    assert_eq!(fs::read(&prepared[0]).unwrap(), prepared_bytes);
    assert_eq!(
        evidence.each_ref().map(|path| fs::read(path).unwrap()),
        originals
    );
    assert_eq!(recovery_receipts(&fixture.copy).len(), 2);
}

fn stopped_fixture() -> (Fixture, Owner) {
    let mut fixture = Fixture::new(true);
    let mut historical = identity::current().unwrap();
    historical["started"] = json!("1");
    fixture.bind_controller(historical);
    let owner = fixture.owner().unwrap();
    (fixture, owner)
}

fn reclaim(fixture: &Fixture, owner: &Owner, checkpoint: &dyn Fn() -> Result<()>) -> Result<Value> {
    copy_recover::reclaim(&fixture.copy, owner, &fixture.owner_binding, checkpoint)
}

fn seed_ledger(fixture: &Fixture) {
    let ledger = fixture.copy.join("ledger");
    fs::write(ledger.join("events.ndjson"), b"original journal\n").unwrap();
    fs::write(ledger.join("head.json"), br#"{"original":"head"}"#).unwrap();
}

fn copy_evidence(fixture: &Fixture) -> [PathBuf; 6] {
    [
        fixture.copy.join("ledger/events.ndjson"),
        fixture.copy.join("ledger/head.json"),
        fixture.copy.join("session.json"),
        PathBuf::from(fixture.owner_binding["path"].as_str().unwrap()),
        PathBuf::from(fixture.source_binding["path"].as_str().unwrap()),
        PathBuf::from(fixture.initialized_binding["path"].as_str().unwrap()),
    ]
}

fn directory_identity(path: &Path) -> (u64, u64) {
    common::device_inode_for(path, &fs::symlink_metadata(path).unwrap()).unwrap()
}

fn record_identity(record: &Value) -> (u64, u64) {
    (
        record["device"].as_u64().unwrap(),
        record["inode"].as_u64().unwrap(),
    )
}

fn directory_members(path: &Path) -> BTreeSet<OsString> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect()
}

fn recovery_receipts(copy: &Path) -> Vec<PathBuf> {
    directory_members(copy)
        .into_iter()
        .filter(|name| {
            name.to_str().is_some_and(|name| {
                name.starts_with("copy-lock-recovery-") && name.ends_with(".json")
            })
        })
        .map(|name| copy.join(name))
        .collect()
}

fn assert_recovered(fixture: &Fixture, result: &Value, locks: Value) {
    assert_eq!(result["status"], "owned_locks_released");
    assert_eq!(result["locks"], locks);
    assert_eq!(result["remote_writes"], 0);
    assert_eq!(result["remote_writes_reconciled"], false);
    assert_eq!(result["requires_reconcile"], true);
    assert_eq!(result["target_ready"], false);
    let saved = common::read_bound(&fixture.backend, &result["receipt"], "回收结果收据").unwrap();
    let mut expected = result.clone();
    expected.as_object_mut().unwrap().remove("receipt");
    assert_eq!(saved, expected);
}
