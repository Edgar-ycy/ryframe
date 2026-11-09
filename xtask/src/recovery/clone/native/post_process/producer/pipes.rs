//! 有界管道只交换受管生产者的单次授权或顺序 NDJSON。
use std::{
    io::{BufRead, BufReader, Read, Write},
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::Result;

pub(super) const FRAME_LIMIT: usize = 1024 * 1024;
const EXISTING_LIMIT: usize = 16 * 1024 * 1024;
type Event = std::result::Result<Vec<u8>, &'static str>;

struct WriteRequest {
    bytes: Vec<u8>,
    close: bool,
    deadline: Instant,
}

pub(super) struct Pipes {
    send: SyncSender<WriteRequest>,
    written: Receiver<std::io::Result<()>>,
    received: Receiver<Event>,
    threads: [JoinHandle<()>; 2],
}

impl Pipes {
    pub(super) fn start(
        mut input: Box<dyn Write + Send>,
        output: Box<dyn Read + Send>,
        session: bool,
    ) -> Result<Self> {
        let (send, pending) = mpsc::sync_channel::<WriteRequest>(1);
        let (ack, written) = mpsc::sync_channel(1);
        let (responses, received) = mpsc::sync_channel(1);
        let writer = thread::Builder::new()
            .name("post-producer-write".into())
            .spawn(move || {
                while let Ok(request) = pending.recv() {
                    let result = if Instant::now() < request.deadline {
                        input.write_all(&request.bytes).and_then(|()| input.flush())
                    } else {
                        Err(std::io::ErrorKind::TimedOut.into())
                    };
                    let failed = result.is_err();
                    if ack.send(result).is_err() || failed || request.close {
                        break;
                    }
                }
            })?;
        let reader = thread::Builder::new()
            .name("post-producer-read".into())
            .spawn(move || {
                if session {
                    read_session(BufReader::new(output), responses);
                } else {
                    let _ = responses.send(read_existing(output));
                }
            })?;
        Ok(Self {
            send,
            written,
            received,
            threads: [writer, reader],
        })
    }

    pub(super) fn exchange(
        &self,
        value: &Value,
        close: bool,
        deadline: Instant,
    ) -> Result<Vec<u8>> {
        let bytes = encode(value)?;
        if Instant::now() >= deadline {
            return Err("生产者授权已超过写入期限；禁止发送".into());
        }
        if !matches!(self.received.try_recv(), Err(TryRecvError::Empty)) {
            return Err("生产者在授权前退出或返回未请求的内容；结果未知，禁止重放".into());
        }
        self.send
            .try_send(WriteRequest {
                bytes,
                close,
                deadline,
            })
            .map_err(|_| "生产者请求管道不可用；结果未知，禁止重放")?;
        receive(&self.written, deadline)?
            .map_err(|_| "生产者请求写入未确认；结果未知，禁止重放")?;
        Ok(receive(&self.received, deadline)??)
    }

    pub(super) fn finish(self) -> Result<()> {
        self.finish_until(Instant::now() + Duration::from_secs(2))
    }

    pub(super) fn finish_until(self, deadline: Instant) -> Result<()> {
        let Self {
            send,
            written,
            received,
            threads,
        } = self;
        drop((send, written, received));
        for worker in threads {
            while !worker.is_finished() {
                if Instant::now() >= deadline {
                    return Err("生产者管道未在期限内结束；保留原证据".into());
                }
                thread::sleep(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(10)),
                );
            }
            worker.join().map_err(|_| "生产者管道线程未正常完成")?;
        }
        Ok(())
    }
}

fn receive<T>(receiver: &Receiver<T>, deadline: Instant) -> Result<T> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or("生产者管道超时；结果未知，禁止重放")?;
    receiver
        .recv_timeout(remaining)
        .map_err(|_| "生产者管道超时或关闭；结果未知，禁止重放".into())
}

fn encode(value: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
    if bytes.len() >= FRAME_LIMIT {
        return Err("生产者授权超过单帧限制".into());
    }
    bytes.push(b'\n');
    Ok(bytes)
}

fn read_session(mut input: impl BufRead, responses: SyncSender<Event>) {
    loop {
        let result = read_frame(&mut input);
        let failed = result.is_err();
        if responses.send(result).is_err() || failed {
            break;
        }
    }
}

fn read_frame(input: &mut impl BufRead) -> Event {
    let mut bytes = Vec::new();
    input
        .take((FRAME_LIMIT + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| "生产者响应读取失败；结果未知，禁止重放")?;
    if bytes.len() > FRAME_LIMIT || !bytes.ends_with(b"\n") {
        return Err("生产者响应缺失、超限或截断；结果未知，禁止重放");
    }
    Ok(bytes)
}

fn read_existing(input: impl Read) -> Event {
    let mut bytes = Vec::new();
    input
        .take((EXISTING_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "既有数据验证输出读取失败；结果未知，禁止重放")?;
    if bytes.is_empty() || bytes.len() > EXISTING_LIMIT {
        return Err("既有数据验证输出缺失或超限；结果未知，禁止重放");
    }
    Ok(bytes)
}
