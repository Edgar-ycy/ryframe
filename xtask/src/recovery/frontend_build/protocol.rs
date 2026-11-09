//! 正式前端 consumer 的固定有界 stdin 入口；失败不输出部分成功 JSON。
use super::{fail, files, model, source};
use crate::{Result, check::consumer_context};
use serde_json::{Value, json};
use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().map(|arg| arg.as_os_str()) != Some(OsStr::new("__frontend-build-contract")) {
        return None;
    }
    let result = (|| {
        if args.len() != 3 || args[1] != OsStr::new("--frontend-dir") {
            return Err(fail("参数必须是 --frontend-dir 和明确绝对路径"));
        }
        let frontend = PathBuf::from(&args[2]);
        if !frontend.is_absolute() {
            return Err(fail("frontend-dir 必须是绝对普通目录"));
        }
        let mut raw = Vec::new();
        std::io::stdin()
            .lock()
            .take(files::LIMIT + 1)
            .read_to_end(&mut raw)
            .map_err(|_| fail("stdin 读取失败"))?;
        let authority = consumer_context::existing_origin(&crate::workspace::root_dir())?;
        let contract = execute(&frontend, &raw)?;
        let value = json!({"frontend_domains":contract.frontend,"backend_domains":contract.backend,
            "frontend_build":contract.proof.summary()});
        let encoded = serde_json::to_vec(&value).map_err(|_| fail("响应编码失败"))?;
        authority.verify()?;
        contract.proof.verify()?;
        let mut output = std::io::stdout().lock();
        output
            .write_all(&encoded)
            .and_then(|()| output.flush())
            .map_err(|_| fail("响应写入失败"))?;
        drop(contract);
        drop(authority);
        Ok(())
    })();
    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!(
                "FrontendBuildRejected：{}；请核对当前绑定工具、稳定的源码/配置/产物并重新构建 v3 证明，保留失败现场。",
                super::reason(error.as_ref())
            );
            1
        }
    })
}
struct Contract {
    frontend: Vec<Value>,
    backend: Vec<Value>,
    proof: super::Proof,
}

fn execute(root: &Path, raw: &[u8]) -> Result<Contract> {
    let value = files::parse(raw)?;
    model::exact(&value, &["frontend_inventories", "backend_inventories"])?;
    let frontend = domains(&value["frontend_inventories"], "frontend")?;
    let backend = domains(&value["backend_inventories"], "backend")?;
    let proof = super::validate(root, None, None)?;
    proof.verify()?;
    Ok(Contract {
        frontend,
        backend,
        proof,
    })
}
fn domains(values: &Value, repository: &str) -> Result<Vec<Value>> {
    values
        .as_array()
        .ok_or_else(|| fail("来源 inventories 必须是数组"))?
        .iter()
        .map(|inventory| {
            source::protocol::request(
                &json!({"action":"domains","repository":repository,"inventory":inventory}),
            )
        })
        .collect()
}
