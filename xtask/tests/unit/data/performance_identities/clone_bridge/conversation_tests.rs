use super::*;
use crate::data::performance_identities::clone_bridge::{Controller, Conversation, model};
use std::{
    io::{Cursor, Write},
    path::PathBuf,
};

async fn initialized() -> (Conversation, Rc<RefCell<Observed>>) {
    let plan = plan();
    let identity = system_identity(&plan);
    let observed = Rc::new(RefCell::new(Observed::default()));
    let context = FakeContext {
        observed: observed.clone(),
        scripts: VecDeque::from([script(
            identity.clone(),
            [(
                "get_auth_context",
                vec![authorization(
                    &identity,
                    true,
                    &["tenant:list", "tenant:edit", "tenant:usage:list"],
                )],
            )],
            Value::Null,
        )]),
    };
    let request = initialization(&plan, identity, true);
    let (controller, _) = quota::Controller::new("plan".into(), plan, Box::new(context), &request)
        .await
        .unwrap();
    (
        Conversation {
            controller: Some(Controller::Quota(controller)),
            current: None,
        },
        observed,
    )
}
fn registration() -> model::Registration {
    model::Registration {
        run: PathBuf::from("unused"),
        attempt: 1,
        family: "quota",
        mode: "plan".into(),
        plan_path: PathBuf::from("unused"),
        plan_sha256: "a".repeat(64),
    }
}

#[test]
fn interrupted_malformed_oversize_and_duplicate_sequence_inputs_always_close_owned_session() {
    runtime().block_on(async {
        for (bytes, success) in [
            (Vec::new(), true),
            (b"{\n".to_vec(), false),
            (b"{\"id\":1}".to_vec(), false),
            (b"{\"id\":1,\"id\":2}\n".to_vec(), false),
            (vec![b' '; 1024 * 1024 + 1], false),
            (b"{\"id\":2,\"operation\":\"get\"}\n".to_vec(), false),
        ] {
            let (mut conversation, observed) = initialized().await;
            let result = conversation
                .run(
                    Path::new("unused"),
                    &registration(),
                    &mut Cursor::new(bytes),
                    &mut Vec::new(),
                )
                .await;
            assert_eq!(result.is_ok(), success);
            assert_eq!(observed.borrow().closed, 1);
            assert_eq!(observed.borrow().requests.len(), 1);
        }
    });
}

struct InterruptedInput;
impl std::io::Read for InterruptedInput {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::ErrorKind::Interrupted.into())
    }
}
impl std::io::BufRead for InterruptedInput {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        Err(std::io::ErrorKind::Interrupted.into())
    }
    fn consume(&mut self, _: usize) {}
}

#[test]
fn interrupted_input_channel_closes_owned_session_before_returning_error() {
    runtime().block_on(async {
        let (mut conversation, observed) = initialized().await;
        let result = conversation
            .run(
                Path::new("unused"),
                &registration(),
                &mut InterruptedInput,
                &mut Vec::new(),
            )
            .await;
        assert!(result.is_err());
        assert_eq!(observed.borrow().closed, 1);
        assert_eq!(observed.borrow().requests.len(), 1);
    });
}

#[test]
fn missing_current_seed_proof_blocks_business_request_but_does_not_block_explicit_close() {
    runtime().block_on(async {
        for (operation, success, closes) in [("get", false, 1), ("close", true, 2)] {
            let (mut conversation, observed) = initialized().await;
            let bytes = format!("{{\"id\":1,\"operation\":\"{operation}\"}}\n").into_bytes();
            let result = conversation
                .run(
                    Path::new("unused"),
                    &registration(),
                    &mut Cursor::new(bytes),
                    &mut Vec::new(),
                )
                .await;
            assert_eq!(result.is_ok(), success);
            assert_eq!(observed.borrow().closed, closes);
            assert_eq!(observed.borrow().requests.len(), 1);
        }
    });
}

struct ClosedOutput;
impl Write for ClosedOutput {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "测试输出已关闭",
        ))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn lost_parent_output_still_closes_sessions_and_reports_unknown_result_without_retry() {
    runtime().block_on(async {
        let (mut conversation, observed) = initialized().await;
        let result = conversation
            .run(
                Path::new("unused"),
                &registration(),
                &mut Cursor::new(b"{\"id\":1,\"operation\":\"close\"}\n"),
                &mut ClosedOutput,
            )
            .await;
        assert!(result.is_err());
        assert_eq!(observed.borrow().closed, 2);
        assert_eq!(observed.borrow().requests.len(), 1);
    });
}

#[test]
fn simultaneous_protocol_and_logout_failure_retains_both_failures() {
    runtime().block_on(async {
        let (mut conversation, observed) = initialized().await;
        observed.borrow_mut().fail_close = true;
        let error = conversation
            .run(
                Path::new("unused"),
                &registration(),
                &mut Cursor::new(b"{\n"),
                &mut Vec::new(),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("JSON"));
        assert!(error.contains("身份会话关闭也未完成"));
        assert_eq!(observed.borrow().closed, 1);
    });
}
