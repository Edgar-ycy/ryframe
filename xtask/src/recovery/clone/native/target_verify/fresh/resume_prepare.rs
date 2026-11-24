//! 显式续作先记录完整旧文件绑定；错误保留 intent 与原现场，不自动重放。
use super::super::super::{json as document, resources::common};
use super::registration::{Registration, combine};
use crate::Result;
use crate::utc_timestamp::now_rfc3339;
use serde_json::{Value, json};

pub(super) fn execute(
    registration: &Registration,
    state: &Value,
    observe: impl FnOnce(bool) -> Result<Value>,
) -> Result<Value> {
    let attempt = uuid::Uuid::new_v4().simple().to_string();
    let mut before = serde_json::Map::new();
    let names = document::object(&state["files"], "prepare 精确前像")?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for name in names {
        let binding = common::binding(&registration.target.join(&name))?;
        if state["files"][&name] != json!({"bytes":binding["bytes"],"sha256":binding["sha256"]}) {
            return Err("prepare 前像在发布续作 intent 前变化".into());
        }
        before.insert(name, binding);
    }
    let intent_path = registration
        .target
        .join(format!("resume-prepare-{attempt}.intent.json"));
    document::write_new(
        &intent_path,
        &json!({"format_version":1,"kind":"devex-clone-prepare-resume-intent",
        "attempt":attempt,"at":now_rfc3339(),"request":registration.value["request"],"files_before":before}),
    )?;
    let intent = common::binding(&intent_path)?;
    let result = observe(state["prepared"] == true);
    let (kind, value) = match &result {
        Ok(_) => (
            "confirmed",
            json!({"prepare":common::binding(&registration.target.join("prepare.json"))?,
            "request":common::binding(&registration.target.join("request.json"))?}),
        ),
        Err(_) => (
            "failure",
            json!({"error_type":"FreshPrepareError",
            "reason":"只读 prepare 续作未完成；核对固定输入与完整现场，不自动重放"}),
        ),
    };
    let mut value = value.as_object().ok_or("prepare 续作结果无效")?.clone();
    value.extend(
        document::object(
            &json!({"format_version":1,"kind":format!("devex-clone-prepare-resume-{kind}"),
        "attempt":attempt,"at":now_rfc3339(),"intent":intent,"remote_writes":0}),
            "prepare 续作终态",
        )?
        .clone(),
    );
    let check = (|| {
        if common::binding(&intent_path)? != intent {
            return Err("prepare 续作 intent 在观察期间变化".into());
        }
        document::write_new(
            &registration
                .target
                .join(format!("resume-prepare-{attempt}.{kind}.json")),
            &Value::Object(value),
        )
    })();
    combine(result, check)
}
