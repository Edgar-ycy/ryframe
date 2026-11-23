use super::*;

#[test]
fn changed_tools_reuse_original_product_source_but_never_changed_context_or_product() {
    let fixture = Fixture::new();
    let (path, _) = fixture.restore_inputs();
    let receipt = read_json(&path);
    let original = receipt["sources"]["full"]["source"].clone();
    write(
        &fixture.root.join("tools/python/check.py"),
        "changed tool fixture\n",
    );
    let scope = bridge::scope::Scope::load(&fixture.root, &[], &[]).unwrap();
    assert_ne!(
        source::binding_value(&source::capture(&fixture.root).unwrap().source),
        original
    );
    let fingerprint = original["worktree_fingerprint"].as_str().unwrap();
    assert_eq!(
        scope
            .verify_source(
                &fixture.root,
                &path,
                &receipt,
                fingerprint,
                Some(&receipt["build"])
            )
            .unwrap(),
        original
    );
    let mut changed_context = receipt["build"].clone();
    changed_context["jobs"] = json!("2");
    assert!(
        scope
            .verify_source(
                &fixture.root,
                &path,
                &receipt,
                fingerprint,
                Some(&changed_context)
            )
            .is_err()
    );
    assert!(
        scope
            .verify_source(
                &fixture.root,
                &path,
                &receipt,
                "sha256:wrong",
                Some(&receipt["build"])
            )
            .is_err()
    );
    write(
        &fixture.root.join("docs/data.md"),
        "changed after source verification\n",
    );
    assert!(scope.assert_unchanged().is_err());
    write(
        &fixture.root.join("crates/ryframe-api/src/lib.rs"),
        "pub fn changed_product() {}\n",
    );
    let scope = bridge::scope::Scope::load(&fixture.root, &[], &[]).unwrap();
    assert!(
        scope
            .verify_source(
                &fixture.root,
                &path,
                &receipt,
                fingerprint,
                Some(&receipt["build"])
            )
            .is_err()
    );
}

#[test]
fn explicit_bridge_revalidates_complete_audit_and_rejects_duplicate_build_identity() {
    let fixture = Fixture::new();
    let (build, inventory) = fixture.restore_inputs();
    write(
        &fixture.root.join("tools/python/check.py"),
        "new tool source\n",
    );
    let path = fixture.local.join("audited.json");
    bridge::write(&fixture.root, &build, &inventory, &path).unwrap();
    let binding = json!({"path":path,"sha256":digest::file(&path).unwrap().sha256});
    let scope =
        bridge::scope::Scope::load(&fixture.root, std::slice::from_ref(&binding), &[]).unwrap();
    let receipt = read_json(&build);
    assert_eq!(
        scope
            .verify_source(
                &fixture.root,
                &build,
                &receipt,
                receipt["sources"]["full"]["source"]["worktree_fingerprint"]
                    .as_str()
                    .unwrap(),
                Some(&receipt["build"])
            )
            .unwrap(),
        receipt["sources"]["full"]["source"]
    );
    assert!(
        bridge::scope::Scope::load(&fixture.root, &[binding.clone(), binding.clone()], &[])
            .is_err()
    );
    let second = fixture.local.join("duplicate-audited.json");
    let mut changed = read_json(&path);
    let copied_build = fixture.local.join("copied-build.json");
    write_json(&copied_build, &receipt);
    changed["build"] =
        json!({"path":copied_build,"sha256":digest::file(&copied_build).unwrap().sha256});
    write_json(&second, &changed);
    let duplicate = json!({"path":second,"sha256":digest::file(&second).unwrap().sha256});
    assert!(bridge::scope::Scope::load(&fixture.root, &[binding, duplicate], &[]).is_err());
    changed["audited_source"]["fingerprints"]["support"]["sha256"] = json!("0".repeat(64));
    write_json(&path, &changed);
    assert!(scope.assert_unchanged().is_err());
}

#[test]
fn inherited_audit_keeps_original_build_but_still_checks_current_context_and_frozen_bytes() {
    let fixture = Fixture::new();
    let (build, inventory) = fixture.restore_inputs();
    let audit = fixture.local.join("original-audit.json");
    bridge::write(&fixture.root, &build, &inventory, &audit).unwrap();
    let binding = json!({"path":audit,"sha256":digest::file(&audit).unwrap().sha256});
    write(
        &fixture.root.join("tools/python/check.py"),
        "new current verifier\n",
    );
    assert!(
        bridge::scope::Scope::load(&fixture.root, std::slice::from_ref(&binding), &[]).is_err()
    );
    let scope = bridge::scope::Scope::load(&fixture.root, &[], &[binding]).unwrap();
    let receipt = read_json(&build);
    let original = &receipt["sources"]["full"]["source"];
    let fingerprint = original["worktree_fingerprint"].as_str().unwrap();
    assert_eq!(
        scope
            .verify_source(
                &fixture.root,
                &build,
                &receipt,
                fingerprint,
                Some(&receipt["build"])
            )
            .unwrap(),
        *original
    );
    let mut changed = receipt["build"].clone();
    changed["toolchain"]["cargo"] = json!("different cargo");
    assert!(
        scope
            .verify_source(&fixture.root, &build, &receipt, fingerprint, Some(&changed))
            .is_err()
    );
    let executable = Path::new(receipt["artifacts"]["api"]["executable"].as_str().unwrap());
    fs::write(executable, b"changed frozen artifact").unwrap();
    assert!(scope.assert_unchanged().is_err());
}

#[test]
fn nested_audit_checks_its_own_full_source_even_when_outer_inventory_ignores_it() {
    let outer = Fixture::new();
    let nested = Fixture::at(outer.local.join("nested-product"));
    let (build, inventory) = nested.restore_inputs();
    let audit = nested.local.join("nested-audit.json");
    bridge::write(&nested.root, &build, &inventory, &audit).unwrap();
    let binding = json!({"path":audit,"sha256":digest::file(&audit).unwrap().sha256});
    let before = source::capture(&outer.root).unwrap();
    let scope = bridge::scope::Scope::load(&outer.root, &[binding], &[]).unwrap();
    let receipt = read_json(&build);
    let original = &receipt["sources"]["full"]["source"];
    assert_eq!(
        scope
            .verify_source(
                &outer.root,
                &build,
                &receipt,
                original["worktree_fingerprint"].as_str().unwrap(),
                None
            )
            .unwrap(),
        *original
    );
    write(
        &nested.root.join("tools/python/check.py"),
        "changed nested verifier\n",
    );
    assert_eq!(source::capture(&outer.root).unwrap(), before);
    assert!(scope.assert_unchanged().is_err());
    let sibling = Fixture::new();
    let mut escaped = read_json(&audit);
    escaped["backend_root"] = json!(sibling.root);
    write_json(&audit, &escaped);
    let binding = json!({"path":audit,"sha256":digest::file(&audit).unwrap().sha256});
    assert!(bridge::scope::Scope::load(&outer.root, &[binding], &[]).is_err());
}

#[test]
fn independent_execution_root_keeps_exact_source_rule_without_reusing_coordinator_audit() {
    let coordinator = Fixture::new();
    let execution = Fixture::new();
    let (build, _) = execution.restore_inputs();
    let receipt = read_json(&build);
    let copied = coordinator.local.join("execution-build.json");
    write_json(&copied, &receipt);
    let scope = bridge::scope::Scope::load(&coordinator.root, &[], &[]).unwrap();
    let original = &receipt["sources"]["full"]["source"];
    let fingerprint = original["worktree_fingerprint"].as_str().unwrap();
    assert_eq!(
        scope
            .verify_source(&execution.root, &copied, &receipt, fingerprint, None)
            .unwrap(),
        *original
    );
    write(
        &execution.root.join("tools/python/check.py"),
        "independent execution changed\n",
    );
    assert!(
        scope
            .verify_source(&execution.root, &copied, &receipt, fingerprint, None)
            .is_err()
    );
}

#[cfg(windows)]
#[test]
fn held_scope_uses_nested_audit_and_still_reads_frozen_artifact() {
    let coordinator = Fixture::new();
    let nested = Fixture::at(coordinator.local.join("held-audited-product"));
    for root in [&coordinator.root, &nested.root] {
        git(root, &["checkout", "--quiet", "--detach", "HEAD"]);
    }
    let (build, inventory) = nested.restore_inputs();
    let tool = nested.root.join("tools/python/check.py");
    write(&tool, "audited tool differs from the original build\n");
    let audit = nested.local.join("held-audit.json");
    bridge::write(&nested.root, &build, &inventory, &audit).unwrap();
    let binding = json!({"path":audit,"sha256":digest::file(&audit).unwrap().sha256});
    let mut scope = bridge::scope::Scope::load(&coordinator.root, &[binding], &[]).unwrap();
    scope.hold_windows_sources(&[]).unwrap();
    assert!(fs::write(&tool, b"cannot change the held audited file").is_err());
    scope.assert_unchanged_full().unwrap();

    let receipt = read_json(&build);
    fs::write(
        Path::new(receipt["artifacts"]["api"]["executable"].as_str().unwrap()),
        b"changed artifact",
    )
    .unwrap();
    assert!(scope.assert_unchanged().is_err());
    drop(scope);
    fs::write(&tool, b"write after release").unwrap();
}

#[cfg(windows)]
#[test]
fn held_scope_checks_actual_execution_members_and_rejects_unheld_roots() {
    let coordinator = Fixture::new();
    let execution = Fixture::new();
    let unheld = Fixture::new();
    for root in [&coordinator.root, &execution.root, &unheld.root] {
        git(root, &["checkout", "--quiet", "--detach", "HEAD"]);
    }
    let (build, _) = execution.restore_inputs();
    let receipt = read_json(&build);
    let expected = &receipt["sources"]["full"]["source"];
    let fingerprint = expected["worktree_fingerprint"].as_str().unwrap();
    let mut scope = bridge::scope::Scope::load(&coordinator.root, &[], &[]).unwrap();
    scope.hold_windows_sources(&[&execution.root]).unwrap();
    assert_eq!(
        scope
            .verify_source(&execution.root, &build, &receipt, fingerprint, None)
            .unwrap(),
        *expected,
    );
    let error = scope
        .verify_source(&unheld.root, &build, &receipt, fingerprint, None)
        .unwrap_err();
    assert!(error.to_string().contains("不在已持有的范围内"));
    fs::write(execution.root.join("new-member.txt"), b"new file").unwrap();
    assert!(scope.assert_unchanged().is_err());
    assert!(scope.assert_unchanged_full().is_err());
}

#[test]
fn product_entry_observes_current_toolchain_instead_of_trusting_recorded_context() {
    let fixture = Fixture::new();
    let (path, _) = fixture.restore_inputs();
    let mut receipt = read_json(&path);
    receipt["build"] = bridge::build_context::current(&fixture.root).unwrap();
    write_json(&path, &receipt);
    let scope = bridge::scope::Scope::load(&fixture.root, &[], &[]).unwrap();
    let original = receipt["sources"]["full"]["source"].clone();
    let fingerprint = original["worktree_fingerprint"].as_str().unwrap();
    assert_eq!(
        scope
            .product(&fixture.root, &path, &receipt, fingerprint)
            .unwrap(),
        original
    );
    receipt["build"]["toolchain"]["cargo"] = json!("different recorded toolchain");
    write_json(&path, &receipt);
    assert!(
        scope
            .product(&fixture.root, &path, &receipt, fingerprint)
            .is_err()
    );
}
