use ryframe_generator::command::GenerationBuilder;

fn main() -> Result<(), String> {
    // 可直接在 IDE 中运行；编辑本文件的包名、模型或表名。默认只显示计划。
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let plan = GenerationBuilder::new(root)
        .configuration_directory("config") // 省略此调用时使用工具文件配置的目录。
        .package("order-business")
        .source("sources.legacy")
        .database("control")
        .include_tables(["biz_order", "biz_order_item"])
        .strip_table_prefix("biz_")
        .write(false)
        .plan()?;
    println!("{}", plan.describe());
    // 确认模型与目标后，改为 plan.execute()；写入还需把上面的 write 改为 true。
    Ok(())
}
