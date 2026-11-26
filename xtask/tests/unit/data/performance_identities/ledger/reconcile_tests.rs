use super::*;
use crate::data::performance_identities::test_fixture::Fixture;
use std::fs;

#[test]
fn first_create_reconciliation_resumes_exact_after_without_repeating_or_unlocking_early() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let output = root.join(DIRECTORY);
    fs::create_dir_all(&output).unwrap();
    let operations = state::expected(&fixture.plan).unwrap();
    let user_slot = &operations[2].1;
    let ledger_path = root.join("ledger.json");
    let original = interrupted_ledger(&fixture.plan, &operations);
    evidence::new(&ledger_path, &original).unwrap();
    let before = evidence::encode(&original).unwrap();
    let before_sha256 = evidence::hash(&before);
    let authority = json!({"fixture": "fixed"});
    let reconcile = |authority: &Value, check: &mut dyn FnMut() -> Result<()>| {
        Ledger::reconcile_first_create(
            &fixture.plan,
            &root,
            &output,
            &before_sha256,
            user_slot,
            "103",
            authority,
            check,
        )
    };
    let result = reconcile(&authority, &mut || {
        if output.join(ARCHIVE).try_exists()? {
            return Err("测试在账本替换前中断".into());
        }
        Ok(())
    });
    assert!(result.unwrap_err().to_string().contains("测试在账本替换前"));
    assert_eq!(fs::read(&ledger_path).unwrap(), before);
    assert!(!root.join("lock").exists());
    assert!(reconcile(&json!({"fixture": "other"}), &mut || Ok(())).is_err());

    let result = reconcile(&authority, &mut || {
        if fs::read(&ledger_path)? != before {
            return Err("测试在账本替换后、发布结果前中断".into());
        }
        Ok(())
    });
    assert!(result.unwrap_err().to_string().contains("测试在账本替换后"));
    assert!(!root.join("lock").exists());
    assert!(!output.join(RECEIPT).exists());
    assert_eq!(fs::read(output.join(ARCHIVE)).unwrap(), before);
    let after = fs::read(&ledger_path).unwrap();
    let projected = evidence::parse(&after).unwrap();
    assert_eq!(projected["entries"][2]["id"], "103");
    assert_eq!(projected["users"][user_slot], "103");
    let owner = uuid::Uuid::new_v4().to_string();
    assert!(Ledger::open(&fixture.plan, &root, "apply", &owner).is_err());
    assert!(reconcile(&json!({"fixture": "other"}), &mut || Ok(())).is_err());
    assert_eq!(fs::read(&ledger_path).unwrap(), after);

    let mut unrelated = projected;
    unrelated["users"][user_slot] = json!("104");
    let unrelated = evidence::encode(&unrelated).unwrap();
    fs::write(&ledger_path, &unrelated).unwrap();
    assert!(reconcile(&authority, &mut || Ok(())).is_err());
    assert_eq!(fs::read(&ledger_path).unwrap(), unrelated);
    fs::write(&ledger_path, &after).unwrap();

    let receipt = reconcile(&authority, &mut || Ok(())).unwrap();
    assert_eq!(receipt["remote_writes"], 0);
    assert_eq!(reconcile(&authority, &mut || Ok(())).unwrap(), receipt);
    assert_eq!(fs::read(&ledger_path).unwrap(), after);
    assert_eq!(fs::read(output.join(ARCHIVE)).unwrap(), before);
    assert_eq!(
        fs::read(output.join(RECEIPT)).unwrap(),
        evidence::encode(&receipt).unwrap()
    );
    let mut ordinary = Ledger::open(
        &fixture.plan,
        &root,
        "apply",
        &uuid::Uuid::new_v4().to_string(),
    )
    .unwrap();
    for (operation, slot) in &operations[..3] {
        assert!(ordinary.take(operation, slot).unwrap().is_some());
    }
    assert!(
        ordinary
            .take(&operations[3].0, &operations[3].1)
            .unwrap()
            .is_none()
    );
    ordinary.close().unwrap();
}

fn interrupted_ledger(plan: &Value, operations: &[(String, String)]) -> Value {
    let role_slot = &operations[0].1;
    let user_slot = &operations[2].1;
    let mut original = json!({
        "format_version": 1, "plan_sha256": plan["plan_sha256"],
        "status": "needs-reconciliation",
        "entries": [
            {"operation":operations[0].0,"slot":role_slot,"phase":"confirmed","id":"101"},
            {"operation":operations[1].0,"slot":role_slot,"phase":"confirmed","id":"101"},
            {"operation":OPERATION,"slot":user_slot,"phase":"started"}
        ],
        "roles": {}, "users": {}
    });
    original["roles"][role_slot] = json!("101");
    original
}
