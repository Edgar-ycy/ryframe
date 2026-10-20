use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{ChildStdin, ChildStdout},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc::{Receiver, SyncSender},
    },
};

use serde_json::Value;

pub(crate) const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

pub(crate) struct Line {
    pub(crate) bytes: Vec<u8>,
    pub(crate) wire_bytes: usize,
}

pub(crate) type ReadEvent = Result<Line, &'static str>;

pub(super) struct WriteRequest {
    pub(super) bytes: Vec<u8>,
    pub(super) complete: SyncSender<Result<(), &'static str>>,
}

/// take 限制 read_until 的分配；即使没有换行也最多读取上限加一字节。
pub(crate) fn read_line(reader: &mut impl BufRead) -> Result<Option<Line>, &'static str> {
    let mut bytes = Vec::new();
    let count = reader
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| "mysql-stdout-read")?;
    if count == 0 {
        return Ok(None);
    }
    if count > MAX_RESPONSE_BYTES {
        return Err("mysql-output-limit");
    }
    if bytes.pop() != Some(b'\n') {
        return Err("mysql-output-incomplete");
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    Ok(Some(Line {
        bytes,
        wire_bytes: count,
    }))
}

pub(super) fn read_stdout(stdout: ChildStdout, events: SyncSender<ReadEvent>) {
    let mut reader = BufReader::with_capacity(8192, stdout);
    loop {
        let event = match read_line(&mut reader) {
            Ok(Some(line)) => Ok(line),
            Ok(None) => Err("mysql-eof"),
            Err(error) => Err(error),
        };
        let failed = event.is_err();
        if events.send(event).is_err() || failed {
            break;
        }
    }
}

/// stderr 始终排空；1 仅表示出现字节，2 表示读取失败，不保留原始诊断。
pub(crate) fn drain_stderr(mut stderr: impl Read, failure: Arc<AtomicU8>) {
    let mut bytes = [0; 4096];
    loop {
        match stderr.read(&mut bytes) {
            Ok(0) => break,
            Ok(_) => {
                failure.store(1, Ordering::Release);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                failure.store(2, Ordering::Release);
                break;
            }
        }
    }
}

pub(super) fn write_stdin(mut stdin: ChildStdin, requests: Receiver<WriteRequest>) {
    while let Ok(request) = requests.recv() {
        let result = stdin
            .write_all(&request.bytes)
            .and_then(|()| stdin.flush())
            .map_err(|_| "mysql-stdin-write");
        let failed = result.is_err();
        if request.complete.send(result).is_err() || failed {
            break;
        }
    }
}

pub(crate) struct Response {
    sentinel: String,
    bytes: usize,
    rows: Vec<Value>,
}

impl Response {
    pub(crate) fn new(sentinel: String) -> Self {
        Self {
            sentinel,
            bytes: 0,
            rows: Vec::new(),
        }
    }

    /// 只有完整 sentinel 行可以结束当前查询；失败后调用方必须销毁会话。
    pub(crate) fn accept(&mut self, line: Line) -> Result<bool, &'static str> {
        self.bytes = self
            .bytes
            .checked_add(line.wire_bytes)
            .ok_or("mysql-output-limit")?;
        if self.bytes > MAX_RESPONSE_BYTES {
            return Err("mysql-output-limit");
        }
        let text = std::str::from_utf8(&line.bytes).map_err(|_| "mysql-output-utf8")?;
        if text == self.sentinel {
            return Ok(true);
        }
        let value = crate::recovery::clone::native::json::parse_fragment(text, "MySQL 输出")
            .map_err(|_| "mysql-output-json")?;
        finite_numbers(&value)?;
        self.rows.push(value);
        Ok(false)
    }

    pub(crate) fn finish(self) -> Vec<Value> {
        self.rows
    }
}

fn finite_numbers(value: &Value) -> Result<(), &'static str> {
    match value {
        Value::Number(number) if !number.as_f64().is_some_and(f64::is_finite) => {
            Err("mysql-output-json")
        }
        Value::Array(values) => values.iter().try_for_each(finite_numbers),
        Value::Object(values) => values.values().try_for_each(finite_numbers),
        _ => Ok(()),
    }
}

pub(crate) fn query(sql: &str) -> Result<(Vec<u8>, String), &'static str> {
    if sql.len() > MAX_RESPONSE_BYTES || sql.contains('\0') {
        return Err("mysql-query");
    }
    let sentinel = format!("gate-{}", uuid::Uuid::new_v4().simple());
    let sql = sql.trim_end_matches([';', '\n', '\r']);
    if sql.trim().is_empty() {
        return Err("mysql-query");
    }
    let bytes = format!("{sql};\nSELECT '{sentinel}';\n").into_bytes();
    Ok((bytes, sentinel))
}
