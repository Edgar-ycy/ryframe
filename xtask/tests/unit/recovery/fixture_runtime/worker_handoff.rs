use super::*;

#[test]
fn authorization_stage_rejections_are_exact_and_keep_exit_code() {
    for role in [Role::Api, Role::Worker] {
        for suffix in [
            "监督候选登记前成员监督器已退出",
            "监督候选登记超过120秒期限",
            "候选与实际监督进程不匹配",
            "成员监督器启动未确认",
        ] {
            let mut response = json!({"ok":false,"error":if role == Role::Worker {"WorkerRejected"} else {"ProcessRejected"},
                "message":format!("{}{suffix}", role.label())});
            let error = acceptance(
                role,
                exit(1),
                (&serde_json::to_vec(&response).unwrap(), false),
                (b"", false),
            )
            .unwrap_err();
            assert_eq!(crate::process::failure_exit_code(error.as_ref()), Some(1));
            assert!(error.to_string().contains(suffix));
            response["message"] =
                json!(format!("{}{suffix}; password=private-secret", role.label()));
            assert!(safe_rejection(role, &serde_json::to_vec(&response).unwrap()).is_none());
        }
    }
}

fn exit(code: u32) -> std::process::ExitStatus {
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code)
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw((code as i32) << 8)
    }
}

#[test]
fn rejected_handoff_retains_exit_and_only_fixed_contract_diagnostic() {
    let output = serde_json::to_vec(&json!({"ok":false,"error":"ProcessRejected",
        "message":format!("{}运行合同核验失败；核对冻结来源、配置与产物，保留现场且不启动或停止进程",Role::Api.label())})).unwrap();
    let error = acceptance(
        Role::Api,
        exit(7),
        (&output, false),
        (b"mysql://user:private-secret@host", false),
    )
    .unwrap_err();
    assert_eq!(crate::process::failure_exit_code(error.as_ref()), Some(7));
    assert!(error.to_string().contains("角色运行合同核验失败"));
    assert!(error.to_string().contains("stderr_bytes="));
    assert!(!format!("{error:?}").contains("private-secret"));
}

#[test]
fn untrusted_and_duplicate_response_text_is_never_public() {
    for output in [
        br#"{"ok":false,"error":"ProcessRejected","message":"password=private-secret"}"#.as_slice(),
        br#"{"ok":false,"ok":false,"error":"ProcessRejected","message":"private-secret"}"#
            .as_slice(),
        b"connection-string-private-secret".as_slice(),
    ] {
        let error = acceptance(
            Role::Api,
            exit(1),
            (output, false),
            (b"private-secret", false),
        )
        .unwrap_err();
        assert!(!format!("{error:?}").contains("private-secret"));
        assert!(error.to_string().contains("stdout_sha256="));
    }
    assert!(acceptance(Role::Api, exit(0), (b"", true), (b"", false)).is_err());
    assert!(acceptance(Role::Api, exit(0), (b"", false), (b"", true)).is_err());
}

#[test]
fn preparation_diagnostic_handoff_requires_exact_safe_fields_and_advice() {
    for role in [Role::Api, Role::Worker] {
        for (stage, advice) in [
            ("runtime_input", "核对冻结来源、配置与产物"),
            ("request_contract", "核对原owner/request与运行合同"),
            ("member_spawn", "核对成员创建权限与系统资源"),
            ("member_identity", "核对成员创建身份与退出证据"),
            ("ready_publication", "核对成员ready证据目录与权限"),
        ] {
            let message = format!(
                "{}监督准备失败；阶段={stage}；原因=unknown；系统码=none；原因摘要={}；{advice}；保留现场，显式核对，不重放",
                role.label(),
                "a".repeat(64)
            );
            let response = |message: &str| {
                serde_json::to_vec(&json!({"ok":false,
                "error":if role == Role::Api {"ProcessRejected"} else {"WorkerRejected"},"message":message})).unwrap()
            };
            let bytes = response(&message);
            let error =
                acceptance(role, exit(1), (&bytes, false), (b"private-marker", false)).unwrap_err();
            assert!(error.to_string().contains(&format!("阶段={stage}")));
            assert_eq!(crate::process::failure_exit_code(error.as_ref()), Some(1));
            assert!(!format!("{error:?}").contains("private-marker"));
            for changed in [
                message.clone() + "private-marker",
                message.replace("原因=unknown", "原因=private-marker"),
                message.replace("系统码=none", "系统码=5"),
                message.replace("显式核对", "private-marker"),
            ] {
                assert!(safe_rejection(role, &response(&changed)).is_none());
            }
        }
    }
}
