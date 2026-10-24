use super::*;
use serde_json::json;

fn row(time: &str) -> Value {
    json!({"ProcessId":42,"CreationDate":time})
}
fn expected() -> Value {
    #[cfg(windows)]
    let executable = r"D:\sensitive-fixture\private.exe";
    #[cfg(not(windows))]
    let executable = "/sensitive-fixture/private.exe";
    json!({"pid":42,"started":"116444736000000000","executable":executable})
}

#[test]
fn complete_microseconds_and_both_offsets_preserve_utc_across_days() {
    let cases = [
        ("19700101000000.123456+000", 116444736001234560_u64),
        ("19700101003000.123456+060", 116444718001234560),
        ("19691231233000.123456-060", 116444754001234560),
        ("16010101000000.000001+000", 10),
    ];
    for (text, started) in cases {
        assert_eq!(
            cim(42, &[row(text)]).unwrap(),
            Creation {
                pid: 42,
                started,
                precision_ticks: 10
            }
        );
    }
}

#[test]
fn same_older_and_precision_boundary_do_not_prove_new_generation() {
    let identity = expected();
    let started = 116444736000000000_u64;
    let precision = 10;
    for value in [started - 1, started, started + precision] {
        assert!(
            require_newer(
                &identity,
                &Creation {
                    pid: 42,
                    started: value,
                    precision_ticks: precision
                }
            )
            .is_err()
        );
    }
    assert!(
        require_newer(
            &identity,
            &Creation {
                pid: 42,
                started: started + precision + 1,
                precision_ticks: precision
            }
        )
        .is_ok()
    );
    // CIM 的十 tick 精度不靠舍入放行；低于一微秒的变化不能证明新代次。
    assert!(
        require_newer(
            &identity,
            &Creation {
                pid: 42,
                started: started + 1,
                precision_ticks: 10
            }
        )
        .is_err()
    );
}

#[test]
fn cim_requires_unique_matching_pid_and_complete_known_dmtf_time() {
    let valid = row("19700101000000.123456+000");
    assert!(cim(42, &[]).is_err());
    assert!(cim(42, &[valid.clone(), valid.clone()]).is_err());
    assert!(cim(1, std::slice::from_ref(&valid)).is_err());
    for value in [json!(43), json!(true), json!("42"), json!(42.0)] {
        let mut wrong = valid.clone();
        wrong["ProcessId"] = value;
        assert!(cim(42, &[wrong]).is_err());
    }
    let mut extra = valid;
    extra["unexpected"] = json!(true);
    assert!(cim(42, &[extra]).is_err());
    for time in [
        "19700101000000.123456+***",
        "19700101000000.******+000",
        "19700101000000.123456",
        "19700101000000.12345+000",
        "19700101000000.123456Z000",
        "19700230000000.123456+000",
        "19700101240000.123456+000",
        "16010101000000.000000+000",
        "16010101000000.000000+001",
        "00000101000000.000001+000",
        "99991231235959.999999+000",
        "19700101000000.123456+０00",
    ] {
        assert!(cim(42, &[row(time)]).is_err());
    }
}

#[test]
fn identity_shape_pid_and_precision_fail_closed_without_input_values() {
    let good = expected();
    let observed = Creation {
        pid: 42,
        started: 116444736000000011,
        precision_ticks: 10,
    };
    for case in [
        "pid-type",
        "pid-overflow",
        "started-bool",
        "started-zero",
        "started-width",
        "started-unicode",
        "relative",
        "extra",
    ] {
        let mut invalid = good.clone();
        match case {
            "pid-type" => invalid["pid"] = json!(true),
            "pid-overflow" => invalid["pid"] = json!(u64::from(u32::MAX) + 1),
            "started-bool" => invalid["started"] = json!(true),
            "started-zero" => invalid["started"] = json!("0123"),
            "started-width" => invalid["started"] = json!("1".repeat(19)),
            "started-unicode" => invalid["started"] = json!("１２３"),
            "relative" => invalid["executable"] = json!("sensitive-fixture/private.exe"),
            _ => invalid["unexpected"] = json!("sensitive-fixture"),
        }
        let error = require_newer(&invalid, &observed).unwrap_err().to_string();
        assert!(!error.contains("sensitive-fixture"));
        assert!(!error.contains("116444736000000000"));
    }
    assert!(
        require_newer(
            &good,
            &Creation {
                pid: 43,
                ..observed
            }
        )
        .is_err()
    );
    for precision in [0, 1, 2, 11, u64::MAX] {
        assert!(
            require_newer(
                &good,
                &Creation {
                    pid: 42,
                    started: u64::MAX,
                    precision_ticks: precision
                }
            )
            .is_err()
        );
    }
    let error = cim(42, &[row("sensitive-invalid-value")])
        .unwrap_err()
        .to_string();
    assert!(!error.contains("sensitive-invalid-value"));
}
