use super::*;

fn output(value: Value, code: u32) -> Output {
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code)
    };
    #[cfg(unix)]
    let status = {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw((code as i32) << 8)
    };
    Output {
        status,
        stdout: serde_json::to_vec(&json!({"ok":true,"result":value})).unwrap(),
        stderr: b"private diagnostic must not be echoed".to_vec(),
    }
}

#[test]
fn query_failure_is_distinct_from_invalid_or_ambiguous_observation() {
    let failure = output(
        json!({"status":"query_failed","category":"CimQueryFailed","code":-2147217394_i32}),
        0,
    );
    assert_eq!(
        response(42, &failure).unwrap(),
        CimObservation::QueryFailed {
            code: Some(-2147217394)
        }
    );
    let valid = json!({"ProcessId":42,"CreationDate":"20261025000000.123456+000"});
    let observed = response(42, &output(json!({"status":"observed","rows":[valid]}), 0)).unwrap();
    let CimObservation::Observed(creation) = observed else {
        panic!("成功观察不得归类为查询失败");
    };
    assert_eq!(creation.precision_ticks, 10);
    for rows in [
        json!([]),
        json!([valid, valid]),
        json!([{"ProcessId":43,"CreationDate":"invalid"}]),
    ] {
        assert!(response(42, &output(json!({"status":"observed","rows":rows}), 0)).is_err());
    }
    for value in [
        json!({"status":"observed","rows":[valid],"unknown":true}),
        json!({"status":"unsupported","category":"CimUnsupported"}),
        json!({"status":"query_failed","category":"other","code":null}),
        json!({"status":"query_failed","category":"CimQueryFailed","code":u64::MAX}),
    ] {
        assert!(response(42, &output(value, 0)).is_err());
    }
}

#[test]
fn failed_or_malformed_child_output_never_becomes_a_stop_proof() {
    let mut sample = output(json!({"private":"sensitive response"}), 9);
    let error = response(42, &sample).unwrap_err().to_string();
    assert!(error.contains('9'));
    assert!(!error.contains("sensitive response"));
    assert!(!error.contains("private diagnostic"));
    sample.status = output(Value::Null, 0).status;
    for bytes in [vec![], vec![b'x'; 65537], vec![255], b"{}".to_vec()] {
        sample.stdout = bytes;
        assert!(response(42, &sample).is_err());
    }
}
