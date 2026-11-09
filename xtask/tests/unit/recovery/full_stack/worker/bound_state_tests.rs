use super::{Role, observation::Readiness, observe_bound_state, operation};
use crate::{Result, recovery::clone::native::resources::common};
use serde_json::json;
use std::{cell::Cell, fs};

#[test]
fn optional_readiness_checks_listener_and_stop_observation_skips_both_probes() -> Result<()> {
    let probes = Cell::new(0);
    let listeners = Cell::new(0);
    let probe = || {
        probes.set(probes.get() + 1);
        Ok(false)
    };
    let listener = || {
        listeners.set(listeners.get() + 1);
        Ok(())
    };
    assert!(!Readiness::Skip.sample(Role::Worker, probe, listener)?);
    assert_eq!((probes.get(), listeners.get()), (0, 0));
    assert!(!Readiness::Probe.sample(Role::Worker, probe, listener)?);
    assert_eq!((probes.get(), listeners.get()), (1, 1));
    assert!(
        Readiness::Required
            .sample(Role::Worker, probe, listener)
            .is_err()
    );
    assert_eq!((probes.get(), listeners.get()), (2, 1));
    assert!(Readiness::Probe.sample(Role::Api, || Ok(true), listener)?);
    assert_eq!(listeners.get(), 2);
    Ok(())
}

#[test]
fn strict_ci_api_controls_preserve_pending_lock_before_loading_runtime() -> Result<()> {
    use std::time::{Duration, Instant};

    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local)?;
    let temporary = tempfile::tempdir_in(local)?;
    let directory = operation::canonical(temporary.path())?;
    let role = Role::Api;
    drop(super::guard(role, &directory, Duration::ZERO)?);
    let lock = directory.join(role.lock());
    fs::create_dir(&lock)?;
    let evidence = lock.join("original");
    fs::write(&evidence, b"pending bytes")?;

    for action in ["start", "stop"] {
        let error = super::once(
            role,
            (action, true),
            &directory,
            &directory,
            5.0,
            Instant::now() + Duration::from_secs(10),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("角色尚有未完成控制证据"));
        assert_eq!(fs::read(&evidence)?, b"pending bytes");
        assert!(!directory.join(role.archive()).exists());
    }
    Ok(())
}

#[test]
fn pending_either_role_blocks_observation_before_loading_runtime() -> Result<()> {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local)?;
    let temporary = tempfile::tempdir_in(local)?;
    let directory = operation::canonical(temporary.path())?;
    let environment = directory.join("environment.json");
    operation::write_new(&environment, &json!({"environment":{"APP_ENV":"test"}}))?;
    let binding = common::binding(&environment)?;
    for (role, probe) in [(Role::Api, true), (Role::Worker, false)] {
        let lock = directory.join(role.lock());
        fs::create_dir(&lock)?;
        let error = observe_bound_state(role, &directory, &directory, &binding, probe).unwrap_err();
        assert!(error.to_string().contains("角色尚有未完成控制证据"));
        assert!(lock.is_dir());
    }
    Ok(())
}
