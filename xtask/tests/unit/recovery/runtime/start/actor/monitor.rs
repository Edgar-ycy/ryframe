use super::*;

fn ready() -> Value {
    let executable = std::env::current_exe().unwrap();
    json!({
        "format_version":1,"operation_id":"a".repeat(32),"directory":"same-generation",
        "role":"api","scope_id":"restore-one",
        "supervisor":{"pid":10,"started":"1","executable":executable},
        "monitor":{"pid":11,"started":"2","executable":executable},
    })
}

#[test]
fn monitor_accepts_only_original_operation_and_actual_group_members() -> Result<()> {
    let ready = ready();
    let product = json!({"pid":12,"started":"3","executable":std::env::current_exe()?});
    let tree = json!({
        "runtime_directory":ready["directory"],"role":ready["role"],"scope_id":ready["scope_id"],
        "operation_id":ready["operation_id"],"supervisor":ready["supervisor"],
        "monitor":ready["monitor"],"process":product,
    });
    let members = [ready["supervisor"].clone(), product.clone()];
    verify_tree(&ready, &tree, &members)?;
    assert!(verify_tree(&ready, &tree, &members[..1]).is_err());
    assert!(verify_tree(&ready, &tree, &members[1..]).is_err());
    for (field, changed) in [
        ("runtime_directory", json!("other-generation")),
        ("operation_id", json!("b".repeat(32))),
        ("monitor", json!({"pid":18})),
        ("process", ready["supervisor"].clone()),
    ] {
        let mut changed_tree = tree.clone();
        changed_tree[field] = changed;
        assert!(
            verify_tree(&ready, &changed_tree, &members).is_err(),
            "{field}"
        );
    }
    let mut reused = product;
    reused["started"] = json!("4");
    assert!(verify_tree(&ready, &tree, &[members[0].clone(), reused]).is_err());
    Ok(())
}

#[test]
fn original_member_proof_is_stopped_only_after_observation_and_complete_closure() -> Result<()> {
    use crate::recovery::clone::native::json::exact;

    let ready = ready();
    let members = vec![ready["supervisor"].clone()];
    for (observed, closed, expected_error) in [
        (true, true, Value::Null),
        (false, true, json!("MemberObservationFailed")),
        (true, false, json!("MemberCleanupFailed")),
        (
            false,
            false,
            json!("MemberObservationFailed/MemberCleanupFailed"),
        ),
    ] {
        let proof = completion(ready.clone(), members.clone(), observed, closed);
        exact(
            &proof,
            &[
                "format_version",
                "operation_id",
                "directory",
                "role",
                "scope_id",
                "supervisor",
                "monitor",
                "status",
                "members",
                "error_type",
            ],
            "成员完成证明",
        )?;
        for (key, expected) in ready.as_object().unwrap() {
            assert_eq!(&proof[key], expected);
        }
        assert_eq!(proof["members"], json!(members));
        assert_eq!(proof["error_type"], expected_error);
        assert_eq!(
            proof["status"],
            if observed && closed {
                "stopped"
            } else {
                "failed"
            }
        );
    }
    let error = combine(
        combine(Err("原观察失败".into()), Err("原关闭失败".into()), "原关闭"),
        Err("原发布失败".into()),
        "原发布",
    )
    .unwrap_err()
    .to_string();
    for reason in ["原观察失败", "原关闭失败", "原发布失败"] {
        assert!(error.contains(reason));
    }
    Ok(())
}

#[test]
fn monitor_refuses_an_existing_role_receipt_and_preserves_its_bytes() -> Result<()> {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local)?;
    let directory = tempfile::tempdir_in(local)?;
    let ready = directory.path().join("api-members-ready.json");
    require_absent(&ready)?;
    fs::write(&ready, b"original")?;
    assert!(require_absent(&ready).is_err());
    assert_eq!(fs::read(&ready)?, b"original");
    Ok(())
}
