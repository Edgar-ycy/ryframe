//! 同一缓存 stdin 协议的阶段端口；端口响应不参与错误文本输出。
use super::rejected;
use super::{super::json as doc, stage};
use crate::Result;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::Path;

pub(super) trait Runtime {
    fn call(&mut self, operation: &str, arguments: Value) -> Result<Value>;
}

struct Stream<'a, R, W> {
    input: &'a mut R,
    output: &'a mut W,
    sequence: u64,
}

impl<R: BufRead, W: Write> Runtime for Stream<'_, R, W> {
    fn call(&mut self, operation: &str, arguments: Value) -> Result<Value> {
        self.sequence += 1;
        writeln!(
            self.output,
            "{}",
            json!({"port":operation,"arguments":arguments,"sequence":self.sequence})
        )?;
        self.output.flush()?;
        let mut bytes = Vec::new();
        std::io::Read::take(&mut self.input, 1024 * 1024 + 1).read_until(b'\n', &mut bytes)?;
        if bytes.is_empty() || bytes.len() > 1024 * 1024 {
            return Err(rejected("缓存端口响应为空或超限；保留现场"));
        }
        let value = doc::parse_fragment(std::str::from_utf8(&bytes)?, "缓存阶段端口")?;
        doc::exact(&value, &["sequence", "ok", "result"], "缓存阶段端口")?;
        if value["sequence"].as_u64() != Some(self.sequence) || value["ok"] != true {
            return Err(rejected("缓存阶段端口失败或响应失序；禁止重放"));
        }
        Ok(value["result"].clone())
    }
}

pub(super) fn execute(
    backend: &Path,
    value: &Value,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<Value> {
    doc::exact(
        value,
        &["format_version", "kind", "operation", "arguments"],
        "缓存阶段协议",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "ryframe-cache-process"
        || value["operation"] != "stage"
    {
        return Err(rejected("缓存阶段协议版本或类型无效"));
    }
    let arguments = &value["arguments"];
    if arguments["action"] == "binding" && arguments["parameters"]["owned_lock_identity"].is_null()
    {
        // 当前写控制器的初始化锁不能由跨进程整数冒认；其余查询不再回调 Python。
        return super::binding::execute_stage(backend, arguments, None);
    }
    stage::dispatch(
        backend,
        &value["arguments"],
        &mut Stream {
            input,
            output,
            sequence: 0,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{Runtime, Stream};
    use serde_json::{Value, json};
    use std::io::Cursor;
    #[test]
    fn ordered_ports_reject_duplicate_wrong_sequence_false_and_truncated_replies() {
        for reply in [
            r#"{"sequence":true,"ok":true,"result":null}"#,
            r#"{"sequence":2,"ok":true,"result":null}"#,
            r#"{"sequence":1,"ok":false,"result":"private-secret"}"#,
            r#"{"sequence":1,"sequence":1,"ok":true,"result":null}"#,
            "",
            "{}",
        ] {
            let mut input = Cursor::new(format!("{reply}\n").into_bytes());
            let mut output = Vec::new();
            let mut runtime = Stream {
                input: &mut input,
                output: &mut output,
                sequence: 0,
            };
            let error = runtime.call("observe", json!({})).unwrap_err();
            assert!(!error.to_string().contains("private-secret"));
            let sent: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(sent["sequence"], 1);
        }
    }
    #[test]
    fn port_payload_limit_and_single_request_do_not_retry() {
        let mut input = Cursor::new(vec![b' '; 1024 * 1024 + 1]);
        let mut output = Vec::new();
        let mut runtime = Stream {
            input: &mut input,
            output: &mut output,
            sequence: 0,
        };
        assert!(runtime.call("start", json!({})).is_err());
        assert_eq!(output.iter().filter(|b| **b == b'\n').count(), 1);
        let mut input = Cursor::new(
            b"{\"sequence\":1,\"ok\":true,\"result\":{\"state\":\"stopped\"}}\n".to_vec(),
        );
        let mut output = Vec::new();
        let mut runtime = Stream {
            input: &mut input,
            output: &mut output,
            sequence: 0,
        };
        assert_eq!(
            runtime.call("status", json!({})).unwrap(),
            json!({"state":"stopped"})
        );
    }
    #[test]
    fn published_binding_query_uses_no_callback_messages_or_resource_writes() {
        use crate::recovery::clone::native::{
            json as document, lifecycle::state, resources::common,
        };
        let backend = crate::workspace::root_dir();
        std::fs::create_dir_all(backend.join(".local-tests")).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("cache-binding-")
            .tempdir_in(backend.join(".local-tests"))
            .unwrap();
        document::write_new(
            &directory.path().join("manifest.json"),
            &json!({"kind":"binding-fixture"}),
        )
        .unwrap();
        state::initialize(directory.path()).unwrap();
        let before = common::binding(&directory.path().join("state.json")).unwrap();
        let value = json!({"format_version":1,"kind":"ryframe-cache-process","operation":"stage",
            "arguments":{"action":"binding","directory":directory.path(),"caller":std::process::id(),
                "parameters":{"owned_lock_identity":null,"before":null,"frozen":null}}});
        let mut input = Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        assert_eq!(
            super::execute(&backend, &value, &mut input, &mut output).unwrap(),
            Value::Null
        );
        assert!(output.is_empty());
        assert_eq!(
            common::binding(&directory.path().join("state.json")).unwrap(),
            before
        );
        assert!(!directory.path().join("cache-target").exists());
        assert!(!directory.path().join("run.lock").exists());
    }
}
