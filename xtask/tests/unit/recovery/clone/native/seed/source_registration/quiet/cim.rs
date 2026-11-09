use super::*;

#[test]
fn pid_query_shape_and_typed_rows_reject_invalid_inputs_without_com() {
    assert!(statement(0).is_err());
    assert!(statement(1).is_err());
    assert_eq!(
        statement(u32::MAX).unwrap(),
        "SELECT ProcessId,CreationDate FROM Win32_Process WHERE ProcessId=4294967295"
    );
    let valid = json!({"ProcessId":42,"CreationDate":"19700101000000.123456+000"});
    let row: Row = serde_json::from_value(valid.clone()).unwrap();
    assert_eq!(
        observed(vec![row]),
        json!({"status":"observed","rows":[valid]})
    );
    for invalid in [
        json!({"ProcessId":true,"CreationDate":"raw"}),
        json!({"ProcessId":"42","CreationDate":"raw"}),
        json!({"ProcessId":4294967296_u64,"CreationDate":"raw"}),
        json!({"ProcessId":42,"CreationDate":null}),
        json!({"ProcessId":42,"CreationDate":"raw","extra":true}),
    ] {
        assert!(serde_json::from_value::<Row>(invalid).is_err());
    }
}

#[test]
fn success_preserves_empty_multiple_and_malformed_time_rows_for_timing_validation() {
    let empty = observed(vec![]);
    assert_eq!(empty, json!({"status":"observed","rows":[]}));
    let time = "unparsed-time-with-*";
    let invalid_time = observed(vec![Row {
        process_id: 42,
        creation_date: time.into(),
    }]);
    assert_eq!(invalid_time["status"], "observed");
    assert_eq!(invalid_time["rows"][0]["CreationDate"], time);
    let rows = observed(vec![
        Row {
            process_id: 42,
            creation_date: "first".into(),
        },
        Row {
            process_id: 42,
            creation_date: "second".into(),
        },
    ]);
    assert_eq!(
        rows["rows"],
        json!([{"ProcessId":42,"CreationDate":"first"},{"ProcessId":42,"CreationDate":"second"}])
    );
    assert_eq!(rows["status"], "observed");
}

#[test]
fn query_failures_expose_only_known_hresult_numbers_or_null() {
    for code in [None, Some(-2147217405), Some(-2147024891)] {
        assert_eq!(
            failed(code),
            json!({"status":"query_failed","category":"CimQueryFailed","code":code})
        );
    }
    #[cfg(windows)]
    for (error, expected) in [
        (
            wmi::WMIError::HResultError { hres: -2147217405 },
            Some(-2147217405),
        ),
        (
            wmi::WMIError::SerdeError("private-path/secret-token".into()),
            None,
        ),
        (
            wmi::WMIError::ConvertDatetimeError("private-path/secret-token".into()),
            None,
        ),
    ] {
        let response = failed(error_code(&error));
        assert_eq!(response["code"], json!(expected));
        let text = response.to_string();
        assert!(!text.contains("private-path"));
        assert!(!text.contains("secret-token"));
    }
}
