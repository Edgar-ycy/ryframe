//! 配额与部门阶段共用身份计划、原生会话和有界 stdin 协议。

use super::{execution::Context, native::NativeContext};
use crate::Result;
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    path::Path,
};

#[path = "clone_bridge/department.rs"]
mod department;
#[path = "clone_bridge/model.rs"]
mod model;
#[path = "clone_bridge/quota.rs"]
mod quota;
#[cfg(test)]
#[path = "../../../tests/unit/data/performance_identities/clone_bridge/tests.rs"]
mod tests;

enum Controller {
    Quota(quota::Controller),
    Department(department::Controller),
}

impl Controller {
    async fn initialize(
        registration: &model::Registration,
        plan: Value,
        request: &Value,
    ) -> Result<(Self, Value)> {
        let artifacts = Path::new(
            request["artifacts"]
                .as_str()
                .ok_or("身份桥接 artifacts 无效")?,
        )
        .to_path_buf();
        let mut context: Box<dyn Context> = Box::new(NativeContext::new_at(&plan, artifacts)?);
        let quota = registration.family == "quota";
        let phase = if quota {
            super::inspection::QUOTA_PHASE
        } else {
            "verify"
        };
        context.inspect(phase).await?;
        if quota {
            let (value, receipt) =
                quota::Controller::new(registration.mode.clone(), plan, context, request).await?;
            Ok((Self::Quota(value), receipt))
        } else {
            let (value, receipt) =
                department::Controller::new(registration.mode.clone(), plan, context, request)?;
            Ok((Self::Department(value), receipt))
        }
    }

    async fn execute(&mut self, request: &Value) -> Result<Value> {
        match self {
            Self::Quota(value) => value.execute(request).await,
            Self::Department(value) => value.execute(request).await,
        }
    }

    async fn close(&mut self) -> Result<()> {
        match self {
            Self::Quota(value) => value.close().await,
            Self::Department(value) => value.close().await,
        }
    }
}

pub(super) fn run_hidden_if_requested() -> Option<i32> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.first().map(String::as_str) != Some("__identity-bridge") {
        return None;
    }
    let result = (|| {
        let root = crate::workspace::root_dir();
        let registration = model::Registration::parse(&arguments[1..], &root)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(Conversation::default().run(
            &root,
            &registration,
            &mut std::io::stdin().lock(),
            &mut std::io::stdout().lock(),
        ))
    })();
    Some(match result {
        Ok(()) => 0,
        Err(_) => {
            eprintln!(
                "identity_bridge_failed：原生配额或部门阶段未完成；保留原意图并显式核对，禁止自动重放。"
            );
            1
        }
    })
}

#[derive(Default)]
struct Conversation {
    controller: Option<Controller>,
    current: Option<crate::recovery::clone::native::seed::Current>,
}
impl Conversation {
    async fn run(
        &mut self,
        root: &Path,
        registration: &model::Registration,
        input: &mut impl BufRead,
        output: &mut impl Write,
    ) -> Result<()> {
        let result = self.exchange(root, registration, input, output).await;
        let cleanup = if let Some(value) = &mut self.controller {
            value.close().await
        } else {
            Ok(())
        };
        match (result, cleanup) {
            (Err(primary), Err(_)) => Err(format!(
                "{primary}；身份会话关闭也未完成，保留收据并显式回收，禁止自动重放"
            )
            .into()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
    async fn exchange(
        &mut self,
        root: &Path,
        registration: &model::Registration,
        input: &mut impl BufRead,
        output: &mut impl Write,
    ) -> Result<()> {
        let mut sequence = 0_u64;
        loop {
            let request = match read_request(input) {
                Ok(Some(value)) => value,
                Ok(None) => break Ok(()),
                Err(error) => break Err(error),
            };
            sequence += 1;
            if request["id"].as_u64() != Some(sequence) {
                let _ = send(output, &failure(Value::Null));
                break Err("身份桥接响应序号不连续".into());
            }
            let operation = request["operation"].as_str().unwrap_or("");
            let response = if self.controller.is_none() {
                initialize(root, registration, &request).await.map(|value| {
                    let (created, receipt, proof) = value;
                    self.controller = Some(created);
                    self.current = Some(proof);
                    receipt
                })
            } else if operation == "close" {
                match model::exact(&request, &["id", "operation"], "身份桥接关闭") {
                    Err(error) => Err(error),
                    Ok(_) => match self.controller.as_mut() {
                        Some(value) => value.close().await.map(|()| json!({"closed":true})),
                        None => Err("身份桥接未初始化".into()),
                    },
                }
            } else {
                guarded_execute(
                    root,
                    self.current.as_ref().ok_or("身份桥接缺少固定 seed 代次")?,
                    self.controller.as_mut().ok_or("身份桥接未初始化")?,
                    &request,
                )
                .await
            };
            match response {
                Ok(data) => {
                    if let Err(error) = send(output, &json!({"id":sequence,"ok":true,"data":data}))
                    {
                        break Err(error);
                    }
                }
                Err(_) => {
                    let _ = send(output, &failure(json!(sequence)));
                    break Err("身份桥接操作失败，结果可能未知".into());
                }
            }
            if operation == "close" {
                break Ok(());
            }
        }
    }
}

async fn initialize(
    root: &Path,
    registration: &model::Registration,
    request: &Value,
) -> Result<(
    Controller,
    Value,
    crate::recovery::clone::native::seed::Current,
)> {
    if request["operation"] != "initialize"
        || request["backend"].as_str() != root.to_str()
        || request["run_dir"].as_str() != registration.run.to_str()
        || request["attempt"] != registration.attempt
    {
        return Err("身份桥接首个授权不属于当前生产者".into());
    }
    let plan = registration.authorize(root, request)?;
    let proof = crate::recovery::clone::native::seed::Current::load(root, &registration.run)?;
    let (controller, receipt) = Controller::initialize(registration, plan, request).await?;
    // 初始化若失败也必须关闭已创建会话，不依赖后来来源是否仍有效。
    let mut controller = controller;
    if let Err(error) = proof.check(root) {
        let closed = controller.close().await;
        return match closed {
            Ok(()) => Err(error),
            Err(_) => Err("身份桥接初始化代次变化且关闭未完成；保留生产者收据并显式回收".into()),
        };
    }
    Ok((controller, receipt, proof))
}

async fn guarded_execute(
    root: &Path,
    current: &crate::recovery::clone::native::seed::Current,
    controller: &mut Controller,
    request: &Value,
) -> Result<Value> {
    current.check(root)?;
    let result = controller.execute(request).await?;
    current.check(root)?;
    Ok(result)
}

fn read_request(input: &mut impl BufRead) -> Result<Option<Value>> {
    let mut bytes = Vec::new();
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err("身份桥接请求缺少换行终止".into())
            };
        }
        let length = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if bytes.len() + length > 1024 * 1024 {
            return Err("身份桥接请求超过 1 MiB".into());
        }
        bytes.extend_from_slice(&available[..length]);
        input.consume(length);
        if bytes.last() == Some(&b'\n') {
            let source = std::str::from_utf8(&bytes).map_err(|_| "身份桥接请求不是 UTF-8")?;
            return Ok(Some(crate::recovery::clone::native::json::parse_fragment(
                source,
                "身份桥接请求",
            )?));
        }
    }
}

fn send(output: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn failure(id: Value) -> Value {
    json!({"id":id,"ok":false,"error_type":"NativeBridgeFailure","status":null})
}
