use crate::recovery::clone::native::lifecycle::identity::confirm_live_observation;

#[test]
fn exited_handle_invalidates_successful_identity_observation() {
    assert_eq!(confirm_live_observation(Ok(42), Ok(true)).unwrap(), None);
    assert_eq!(
        confirm_live_observation(Ok(42), Ok(false)).unwrap(),
        Some(42)
    );
}

#[test]
fn image_query_failure_requires_positive_exit_proof() {
    let failed = || Err::<u8, _>("映像查询拒绝访问".into());
    assert_eq!(confirm_live_observation(failed(), Ok(true)).unwrap(), None);
    assert_eq!(
        confirm_live_observation(failed(), Ok(false))
            .unwrap_err()
            .to_string(),
        "映像查询拒绝访问"
    );
}

#[test]
fn failed_exit_query_never_turns_an_observation_into_absence() {
    assert!(confirm_live_observation(Ok(42), Err("退出状态不可读".into())).is_err());
    let error = confirm_live_observation::<u8>(
        Err("映像查询拒绝访问".into()),
        Err("退出状态不可读".into()),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("映像查询拒绝访问"));
    assert!(error.contains("退出状态不可读"));
}
