//! 静态运行代次使用独立生成的命令复核既有原生模型。
use super::{check, commands, inputs::Inputs, path};
use crate::{
    Result,
    recovery::inputs::document::{self, Document},
};
use serde_json::{Value, json};

pub(super) fn verify(
    inputs: &mut Inputs,
    launch: &Document,
    reference: &Value,
    tools: &Value,
) -> Result<()> {
    let root = path(&reference["target"]["runtime_dir"])?;
    let state = inputs.read(&root.join("lifecycle.json"))?;
    let binding = &launch.value["request"]["registration"];
    let generations =
        crate::recovery::clone::native::json::array(&state.value["generations"], "恢复代次集合")?;
    let mut expected = Vec::new();
    for generation in generations {
        let request = &generation["request"];
        check(
            commands::same_path(&root, &path(&request["runtime_directory"])?)?,
            "恢复代理没有绑定同一登记运行目录",
        )?;
        let recorded = json!({"api":generation["roles"]["api"]["command"],"worker":generation["roles"]["worker"]["command"],"frontend":generation["roles"]["frontend"]["command"]});
        let command =
            commands::recorded_spelling(commands::expected(request, tools, inputs)?, &recorded)?;
        protect_receipts(inputs, generation)?;
        expected.push(command);
    }
    super::super::generation::request(
        &json!({"action":"validate-state", "root":root, "binding":binding, "value":state.value, "previous":null, "commands":expected, "number":1}),
    )?;
    let number = launch.value["generation"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("恢复启动代次无效")?;
    let generation = generations
        .get(usize::try_from(number - 1)?)
        .ok_or("恢复启动代次不存在")?;
    check(
        generation["number"] == number
            && generation["status"] == "running"
            && generation["request"] == launch.value["request"]
            && generation["launch"] == document::binding(launch),
        "恢复启动未绑定同一生命周期中的 running 代次",
    )?;
    let directory = path(&launch.value["runtime_directory"])?;
    super::environment::verify_projected(inputs, &launch.value["request"], &directory)?;
    check(
        launch.path == directory.join("runtime-launch.json"),
        "恢复启动路径不属于登记代次",
    )?;
    let recorded = json!({"api":launch.value["processes"]["api"]["command"],"worker":launch.value["processes"]["worker"]["command"],"frontend":launch.value["processes"]["frontend"]["command"]});
    let command = commands::recorded_spelling(
        commands::expected(&launch.value["request"], tools, inputs)?,
        &recorded,
    )?;
    super::super::generation::request(
        &json!({"action":"validate-launch", "root":directory, "binding":binding, "value":launch.value, "previous":null, "commands":command, "number":number}),
    )?;
    let creation = super::super::generation::creation_path(&root);
    inputs.bound(&state.value["creation_intent"])?;
    check(
        creation == path(&state.value["creation_intent"]["path"])?,
        "恢复首次 intent 路径不同",
    )?;
    Ok(())
}

fn protect_receipts(inputs: &mut Inputs, generation: &Value) -> Result<()> {
    let directory = path(&generation["directory"])?;
    if !directory.try_exists()? {
        return Ok(());
    }
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            inputs.read(&path)?;
        }
    }
    Ok(())
}
