use super::*;
use serde_json::json;

fn recorded(started: u64) -> Value {
    json!({"pid":42,"started":started.to_string(),"executable":std::env::current_exe().unwrap()})
}

#[test]
fn kernel_absence_or_strictly_later_generation_never_needs_cim() {
    let expected = recorded(100);
    let mut forbidden = |_| panic!("内核身份充分时不应调用 CIM");
    assert!(verify_with(&expected, &mut |_| Ok(Inspection::Gone), &mut forbidden).is_ok());
    assert!(
        verify_with(
            &expected,
            &mut |_| Ok(Inspection::Present(recorded(101))),
            &mut forbidden
        )
        .is_ok()
    );
    for time in [99, 100] {
        let mut current = recorded(time);
        current["executable"] = json!(std::env::current_exe().unwrap().with_file_name("other.exe"));
        assert!(
            verify_with(
                &expected,
                &mut |_| Ok(Inspection::Present(current.clone())),
                &mut forbidden
            )
            .is_err()
        );
    }
    assert!(
        verify_with(
            &expected,
            &mut |_| Err("未知内核错误".into()),
            &mut forbidden
        )
        .is_err()
    );
}

#[test]
fn denied_kernel_requires_cim_beyond_full_precision_boundary() {
    let expected = recorded(100);
    for (time, accepted) in [
        (99, false),
        (100, false),
        (101, false),
        (110, false),
        (111, true),
    ] {
        let mut calls = Vec::new();
        let result = verify_with(
            &expected,
            &mut |_| Ok(Inspection::AccessDenied),
            &mut |pid| {
                calls.push(pid);
                Ok(CimObservation::Observed(timing::Creation {
                    pid,
                    started: time,
                    precision_ticks: 10,
                }))
            },
        );
        assert_eq!(result.is_ok(), accepted);
        assert_eq!(calls, [42]);
    }
}

#[test]
fn cim_failure_or_invalid_evidence_is_rejected_with_actionable_diagnostics() {
    let expected = recorded(100);
    let error = verify_with(
        &expected,
        &mut |_| Ok(Inspection::AccessDenied),
        &mut |_| {
            Ok(CimObservation::QueryFailed {
                code: Some(-2147024891),
            })
        },
    )
    .unwrap_err()
    .to_string();
    for fact in ["错误码 5", "0x80070005", "Winmgmt", "ROOT\\CIMV2", "禁止"] {
        assert!(error.contains(fact), "{error}");
    }
    assert!(
        verify_with(
            &expected,
            &mut |_| Ok(Inspection::AccessDenied),
            &mut |_| Err("CIM 响应身份不唯一".into())
        )
        .is_err()
    );
    let mut queried = false;
    let mut invalid = expected;
    invalid["pid"] = json!(1);
    assert!(
        verify_with(
            &invalid,
            &mut |_| {
                queried = true;
                Ok(Inspection::Gone)
            },
            &mut |_| panic!("无效身份不得触发 CIM")
        )
        .is_err()
    );
    assert!(!queried);
}
