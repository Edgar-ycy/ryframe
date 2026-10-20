//! 补充完整 serve 循环的多 ticket、握手 EOF 和拒绝路径。
//! 验证每个请求的精确回执、事务配对、错误分类与输入关闭后的完整收尾。

use std::{
    io::{self, BufRead, Read},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
};

use serde_json::{Value, json};

use crate::devex::job_evidence::{protocol::Session, wait};

const SCOPE: &str = "devex-test-scope";
const SERVER: &str = "12345678-1234-1234-1234-123456789abc";
const SESSION_ID: u64 = 17;
const OWNER_SQL: &str = concat!(
    "SELECT JSON_OBJECT('scope',scope_id,'marker',marker,'server',@@server_uuid) ",
    "FROM `devex_control`.ryframe_resource_ownership WHERE resource_kind='control'",
);

#[derive(Clone, Debug, PartialEq)]
enum Observation {
    Query(u64, String),
    Output(Value),
    InputEof,
}

type Trace = Arc<Mutex<Vec<Observation>>>;

fn record(trace: &Trace, observation: Observation) {
    trace.lock().expect("记录内存事件").push(observation);
}

fn binding() -> Value {
    json!({
        "control": {
            "host": "127.0.0.1",
            "port": 3306,
            "database": "devex_control",
            "username": "test",
            "password_env": "DEVEX_TEST_PASSWORD",
            "tls_mode": "required",
        },
        "server_uuid": SERVER,
        "mysql_client": "fixture-client",
    })
}

fn selection(id: &str) -> Value {
    json!({
        "kind": "export",
        "id": id,
        "tenant_id": "tenant-a",
        "job_type": "system.export.execute",
    })
}

fn request(ticket: u64, id: &str) -> Value {
    json!({"ticket": ticket, "selection": selection(id), "timeout_ms": 300_000})
}

fn success(ticket: u64, id: &str, job_id: &str) -> Value {
    json!({
        "ticket": ticket,
        "scope_id": SCOPE,
        "selection": selection(id),
        "valid": true,
        "job_id": job_id,
        "status": "succeeded",
    })
}

fn job(id: &str, job_id: &str) -> Value {
    json!({
        "business_id": id,
        "business_tenant": "tenant-a",
        "business_status": "succeeded",
        "job_id": job_id,
        "job_tenant": "tenant-a",
        "job_type": "system.export.execute",
        "status": "succeeded",
        "claim_sequence": 1,
    })
}

struct FakeSession {
    trace: Trace,
    queries: Vec<String>,
    close_calls: usize,
}

impl FakeSession {
    fn new(trace: &Trace) -> Self {
        Self {
            trace: Arc::clone(trace),
            queries: Vec::new(),
            close_calls: 0,
        }
    }
}

impl Session for FakeSession {
    fn query(&mut self, statement: &str) -> Result<Vec<Value>, &'static str> {
        self.queries.push(statement.to_owned());
        record(
            &self.trace,
            Observation::Query(SESSION_ID, statement.to_owned()),
        );
        match statement {
            "SET SESSION time_zone='+00:00'" | "START TRANSACTION READ ONLY" | "ROLLBACK" => {
                Ok(Vec::new())
            }
            OWNER_SQL => Ok(vec![json!({
                "scope": SCOPE,
                "marker": format!("ryframe-owner:v1:{SCOPE}:control"),
                "server": SERVER,
            })]),
            _ if statement.starts_with("SELECT JSON_OBJECT(") => {
                if statement.contains(" WHERE b.id=101 AND b.tenant_id='tenant-a' LIMIT 2") {
                    Ok(vec![job("101", "201")])
                } else if statement.contains(" WHERE b.id=102 AND b.tenant_id='tenant-a' LIMIT 2") {
                    Ok(vec![job("102", "202")])
                } else {
                    Err("unexpected_fixture_query")
                }
            }
            _ => Err("unexpected_fixture_query"),
        }
    }

    fn close(&mut self) -> Result<(), &'static str> {
        self.close_calls += 1;
        Ok(())
    }
}

/// 接收端仅缓冲内存帧；所有发送端释放后才向 serve 的输入线程报告 EOF。
struct ChannelInput {
    receiver: Receiver<Vec<u8>>,
    buffer: Vec<u8>,
    position: usize,
    eof: bool,
    trace: Trace,
}

fn channel_input(trace: &Trace) -> (SyncSender<Vec<u8>>, ChannelInput) {
    // ready 在输入线程启动前调用；容量二允许回调完整放入两个请求而不阻塞。
    let (sender, receiver) = mpsc::sync_channel(2);
    (
        sender,
        ChannelInput {
            receiver,
            buffer: Vec::new(),
            position: 0,
            eof: false,
            trace: Arc::clone(trace),
        },
    )
}

impl Read for ChannelInput {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let available = self.fill_buf()?;
        let length = available.len().min(output.len());
        output[..length].copy_from_slice(&available[..length]);
        self.consume(length);
        Ok(length)
    }
}

impl BufRead for ChannelInput {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.position == self.buffer.len() && !self.eof {
            match self.receiver.recv() {
                Ok(buffer) => {
                    self.buffer = buffer;
                    self.position = 0;
                }
                Err(_) => {
                    self.eof = true;
                    record(&self.trace, Observation::InputEof);
                }
            }
        }
        if self.eof {
            Ok(&[])
        } else {
            Ok(&self.buffer[self.position..])
        }
    }

    fn consume(&mut self, amount: usize) {
        self.position += amount.min(self.buffer.len() - self.position);
    }
}

fn request_line(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).expect("编码内存请求");
    bytes.push(b'\n');
    bytes
}

fn send_request(sender: &SyncSender<Vec<u8>>, value: &Value) {
    sender
        .send(request_line(value))
        .expect("向输入线程发送请求");
}

fn assert_read_only_snapshots(session: &FakeSession) {
    assert_eq!(session.queries[0], "SET SESSION time_zone='+00:00'");
    assert_eq!(session.queries[1], OWNER_SQL);
    assert_eq!(session.queries.last().map(String::as_str), Some(OWNER_SQL));
    let mut active = false;
    let mut owned = false;
    let mut starts = 0;
    let mut rollbacks = 0;
    for statement in &session.queries[2..session.queries.len() - 1] {
        match statement.as_str() {
            "START TRANSACTION READ ONLY" => {
                assert!(!active);
                active = true;
                owned = false;
                starts += 1;
            }
            OWNER_SQL => {
                assert!(active && !owned);
                owned = true;
            }
            "ROLLBACK" => {
                assert!(active && owned);
                active = false;
                rollbacks += 1;
            }
            _ => assert!(active && owned && statement.starts_with("SELECT JSON_OBJECT(")),
        }
    }
    assert!(!active);
    assert!(starts >= 1);
    assert_eq!(starts, rollbacks);
    assert_eq!(
        session
            .queries
            .iter()
            .filter(|sql| *sql == OWNER_SQL)
            .count(),
        starts + 2
    );
    for id in ["101", "102"] {
        let identity = format!(" WHERE b.id={id} AND b.tenant_id='tenant-a' LIMIT 2");
        assert_eq!(
            session
                .queries
                .iter()
                .filter(|sql| sql.contains(&identity))
                .count(),
            1
        );
    }
    assert!(
        session
            .queries
            .iter()
            .all(|sql| !sql.contains("sys_background_job_attempt"))
    );
    assert_eq!(session.close_calls, 0);
}

fn assert_eof_precedes_final_owner_check(trace: &Trace, expected_outputs: usize) {
    let observations = trace.lock().expect("读取内存事件");
    let eof = observations
        .iter()
        .position(|event| *event == Observation::InputEof)
        .expect("输入通道已报告 EOF");
    assert_eq!(
        observations
            .iter()
            .filter(|event| **event == Observation::InputEof)
            .count(),
        1
    );
    assert_eq!(
        observations
            .iter()
            .filter(|event| matches!(event, Observation::Output(_)))
            .count(),
        expected_outputs
    );
    for (position, event) in observations.iter().enumerate() {
        match event {
            Observation::Query(identity, _) => assert_eq!(*identity, SESSION_ID),
            Observation::Output(_) => assert!(position < eof),
            Observation::InputEof => {}
        }
    }
    assert!(eof < observations.len() - 1);
    assert_eq!(
        observations.last(),
        Some(&Observation::Query(SESSION_ID, OWNER_SQL.to_owned()))
    );
}

#[test]
fn serve_completes_two_distinct_tickets_on_one_session_before_input_closes() {
    let trace = Trace::default();
    let (sender, input) = channel_input(&trace);
    let mut sender = Some(sender);
    let mut session = FakeSession::new(&trace);
    let mut outputs = Vec::new();
    let result = wait::serve(&mut session, &binding(), SCOPE, input, |value| {
        record(&trace, Observation::Output(value.clone()));
        if value.get("ready").is_some() {
            assert!(outputs.is_empty());
            assert_eq!(value, json!({"ready": true, "scope_id": SCOPE}));
            let sender = sender.as_ref().expect("握手期间保持输入开放");
            send_request(sender, &request(1, "101"));
            send_request(sender, &request(2, "102"));
        } else {
            let expected = match value["ticket"].as_u64() {
                Some(1) => success(1, "101", "201"),
                Some(2) => success(2, "102", "202"),
                _ => panic!("出现未登记的 ticket"),
            };
            assert_eq!(value, expected);
            assert!(
                !outputs
                    .iter()
                    .any(|output: &Value| output["ticket"] == value["ticket"])
            );
        }
        outputs.push(value);
        if outputs.len() == 3 {
            // 两份完整成功回执均已校验，才释放唯一发送端。
            drop(sender.take());
        }
        Ok(())
    });
    drop(sender.take());
    assert_eq!(result, Ok(()));
    assert_eq!(
        outputs,
        vec![
            json!({"ready": true, "scope_id": SCOPE}),
            success(1, "101", "201"),
            success(2, "102", "202"),
        ]
    );
    assert_read_only_snapshots(&session);
    assert_eof_precedes_final_owner_check(&trace, 3);
}

#[test]
fn serve_accepts_immediate_eof_after_ready_and_checks_owner_again() {
    let trace = Trace::default();
    let (sender, input) = channel_input(&trace);
    let mut sender = Some(sender);
    let mut session = FakeSession::new(&trace);
    let mut outputs = Vec::new();
    let result = wait::serve(&mut session, &binding(), SCOPE, input, |value| {
        assert_eq!(value, json!({"ready": true, "scope_id": SCOPE}));
        assert!(outputs.is_empty());
        record(&trace, Observation::Output(value.clone()));
        outputs.push(value);
        drop(sender.take());
        Ok(())
    });
    drop(sender.take());
    assert_eq!(result, Ok(()));
    assert_eq!(outputs, vec![json!({"ready": true, "scope_id": SCOPE})]);
    assert_eq!(
        session.queries,
        vec!["SET SESSION time_zone='+00:00'", OWNER_SQL, OWNER_SQL]
    );
    assert_eq!(session.close_calls, 0);
    assert_eof_precedes_final_owner_check(&trace, 1);
}

fn assert_serve_rejects_frame(bytes: &[u8], reason: &'static str) {
    let trace = Trace::default();
    let (sender, input) = channel_input(&trace);
    let mut sender = Some(sender);
    let mut session = FakeSession::new(&trace);
    let mut outputs = Vec::new();
    let result = wait::serve(&mut session, &binding(), SCOPE, input, |output| {
        assert_eq!(output, json!({"ready": true, "scope_id": SCOPE}));
        assert!(outputs.is_empty());
        outputs.push(output);
        let sender = sender.take().expect("握手只发生一次");
        sender.send(bytes.to_vec()).expect("发送待拒绝的内存帧");
        // 非法帧先于 EOF；accept 或解析失败必须返回固定错误，不能被 EOF 成功吞掉。
        drop(sender);
        Ok(())
    });
    drop(sender.take());
    assert_eq!(result, Err(reason));
    assert_eq!(outputs, vec![json!({"ready": true, "scope_id": SCOPE})]);
    assert_eq!(
        session.queries,
        vec!["SET SESSION time_zone='+00:00'", OWNER_SQL]
    );
    assert_eq!(session.close_calls, 0);
}

#[test]
fn serve_rejects_unknown_fields_and_invalid_request_shapes_with_fixed_categories() {
    let mut unknown = request(1, "101");
    unknown["unknown"] = json!(true);
    let mut boolean_ticket = request(1, "101");
    boolean_ticket["ticket"] = json!(true);
    let mut short_timeout = request(1, "101");
    short_timeout["timeout_ms"] = json!(999);
    for value in [unknown, boolean_ticket, short_timeout] {
        assert_serve_rejects_frame(&request_line(&value), "invalid_wait_request");
    }
    assert_serve_rejects_frame(
        &request_line(&request(1, "1 OR 1=1")),
        "invalid_job_identifier",
    );
}

#[test]
fn serve_preserves_malformed_and_unterminated_frame_error_categories() {
    for bytes in [b"not-json\n".as_slice(), b"\xff\n".as_slice()] {
        assert_serve_rejects_frame(bytes, "invalid_wait_request");
    }
    assert_serve_rejects_frame(b"{}", "wait_input_limit");
}

#[test]
fn serve_rejects_reused_ticket_after_completion_without_succeeding_the_second_id() {
    let trace = Trace::default();
    let (sender, input) = channel_input(&trace);
    let mut sender = Some(sender);
    let mut session = FakeSession::new(&trace);
    let mut outputs = Vec::new();
    let result = wait::serve(&mut session, &binding(), SCOPE, input, |value| {
        if outputs.is_empty() {
            assert_eq!(value, json!({"ready": true, "scope_id": SCOPE}));
            send_request(sender.as_ref().expect("输入开放"), &request(1, "101"));
        } else {
            assert_eq!(outputs.len(), 1);
            assert_eq!(value, success(1, "101", "201"));
            let sender = sender.take().expect("收到首份成功回执后才发送重复编号");
            send_request(&sender, &request(1, "102"));
            drop(sender);
        }
        outputs.push(value);
        Ok(())
    });
    drop(sender.take());
    assert_eq!(result, Err("invalid_wait_request"));
    assert_eq!(
        outputs,
        vec![
            json!({"ready": true, "scope_id": SCOPE}),
            success(1, "101", "201"),
        ]
    );
    assert_eq!(
        session
            .queries
            .iter()
            .filter(|sql| sql.contains(" WHERE b.id=101 "))
            .count(),
        1
    );
    assert!(
        !session
            .queries
            .iter()
            .any(|sql| sql.contains(" WHERE b.id=102 "))
    );
    assert_eq!(
        session
            .queries
            .iter()
            .filter(|sql| *sql == "START TRANSACTION READ ONLY")
            .count(),
        1
    );
    assert_eq!(
        session
            .queries
            .iter()
            .filter(|sql| *sql == "ROLLBACK")
            .count(),
        1
    );
    assert_eq!(session.queries.last().map(String::as_str), Some("ROLLBACK"));
    assert_eq!(session.close_calls, 0);
}
