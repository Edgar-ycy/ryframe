use super::{manifest, model::*, publication, receipts, responses};
use crate::{Result, recovery::clone::native::json as strict};
use serde_json::{Value, json};
use std::{env, io::Read};
pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_none_or(|s| s != "__fixture-browser") {
        return None;
    }
    let result = (|| {
        check(args.len() == 1, "浏览器协议只接受标准输入")?;
        let mut raw = Vec::new();
        std::io::stdin()
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut raw)?;
        execute(&raw)
    })();
    let (code, response) = match result {
        Ok(value) => (0, json!({"ok":true,"result":{"value":value}})),
        Err(error) => {
            let reason = error
                .downcast_ref::<Rejected>()
                .map_or("依赖调用、文件访问或证据核验失败", |error| {
                    error.0.as_str()
                });
            (
                2,
                json!({"ok":false,"error":{"code":"invalid_fixture_browser","message":format!("浏览器夹具操作未确认：{reason}；核对当前阶段、绑定文件、原进程收据与待发布证据，保留现场且禁止自动重放")}}),
            )
        }
    };
    println!("{response}");
    Some(code)
}
pub(super) fn execute(raw: &[u8]) -> Result<Value> {
    check(
        !raw.is_empty() && raw.len() <= 16 * 1024 * 1024,
        "浏览器请求为空或超过16MiB",
    )?;
    let v = strict::parse_fragment(std::str::from_utf8(raw)?, "浏览器协议")
        .map_err(|_| "浏览器协议无效")?;
    strict::exact(&v, &["operation", "arguments", "write"], "浏览器协议")?;
    let op = text(&v["operation"])?;
    let a = &v["arguments"];
    check(
        v["write"].as_bool()
            == if op == "execute" {
                a["write"].as_bool()
            } else {
                Some(matches!(op, "terminal" | "tool-publish"))
            },
        "浏览器写入授权与操作不一致",
    )?;
    match op {
        "execute" => {
            let mut api = super::dependency::Python::new(&path(&a["backend_dir"])?)?;
            super::execution::execute(&mut api, a)
        }
        "manifest" => manifest::capture(
            &path(&a["root"])?,
            &path(&a["parent"])?,
            (
                a["maximum_files"].as_u64().ok_or("文件数上限无效")?,
                a["maximum_bytes"].as_u64().ok_or("字节数上限无效")?,
            ),
        ),
        "manifest-verify" => {
            manifest::verify(&a["manifest"], &path(&a["root"])?, &path(&a["parent"])?)
        }
        "manifest-assert" => {
            manifest::assert_unchanged(&a["snapshot"])?;
            Ok(Value::Null)
        }
        "business-tests" => {
            receipts::business(&path(&a["path"])?, text(&a["server"])?, text(&a["run_id"])?)
        }
        "login-budget" => receipts::budget(&path(&a["path"])?, &a["binding"]),
        "redacted-log" => {
            let secrets = serde_json::from_value::<Vec<String>>(a["secrets"].clone())?;
            read_log(&path(&a["path"])?, &a["descriptor"], &secrets)
        }
        "preview-responses" => {
            responses::preview(&path(&a["path"])?, &a["binding"], &a["manifest"])
        }
        "tool-capture" => publication::tool_capture(&path(&a["frontend"])?),
        "tool-assert" => {
            publication::tool_assert(&a["capture"])?;
            Ok(Value::Null)
        }
        "tool-publish" => publication::tool_publish(&a["capture"], &a["configuration"]),
        "terminal" => {
            publication::terminal(&path(&a["directory"])?, &path(&a["target"])?, &a["value"])?;
            Ok(Value::Null)
        }
        _ => Err("未知浏览器私有操作".into()),
    }
}
