//! RyFrame 离线代码生成器命令行入口。

use std::process::ExitCode;

use ryframe_generator::{
    cli::{CliEnvironment, CliOptions, help_text, parse_cli},
    generate, write_to_disk,
};
use ryframe_kernel::AppError;
use sea_orm::Database;

#[tokio::main]
async fn main() -> ExitCode {
    let environment = CliEnvironment::from_process();
    let options = match parse_cli(std::env::args().skip(1), environment) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{}", help_text());
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("参数错误：{error}");
            eprintln!(
                "请运行 `cargo run -p ryframe-generator --bin ryframe-gen -- --help` 查看帮助。"
            );
            return ExitCode::from(2);
        }
    };

    match run(options).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(options: CliOptions) -> Result<(), String> {
    println!("数据库目标：{}", options.database_url.target());
    println!("数据表：{}", options.generate.tables.join(", "));
    println!(
        "运行模式：{}",
        if options.write {
            "写入工作区"
        } else {
            "预览（dry-run，不写盘）"
        }
    );

    let database = Database::connect(options.database_url.expose())
        .await
        .map_err(|_| {
            format!(
                "连接数据库失败（目标：{}）；详细连接错误已隐藏，避免泄露凭据。",
                options.database_url.target()
            )
        })?;
    let files = generate(&database, &options.generate)
        .await
        .map_err(|error| safe_app_error("生成代码失败", &error))?;

    println!("生成结果共 {} 个文件：", files.len());
    for file in &files {
        println!("  -> {}", file.path);
    }

    if !options.write {
        println!("dry-run 完成，未写入任何文件；确认结果后可显式增加 `--write`。");
        return Ok(());
    }

    let workspace_root = std::env::current_dir()
        .map_err(|_| "无法确定当前工作区目录，未写入任何文件。".to_owned())?;
    let report = write_to_disk(&files, &workspace_root, options.generate.overwrite)
        .await
        .map_err(|error| safe_app_error("写入生成文件失败", &error))?;

    println!("写入完成：{} 个文件。", report.written.len());
    for path in &report.written {
        println!("  -> {path}");
    }
    if !report.skipped.is_empty() {
        println!("未覆盖并跳过：{} 个文件。", report.skipped.len());
        for path in &report.skipped {
            println!("  == {path}");
        }
    }
    Ok(())
}

fn safe_app_error(action: &str, error: &AppError) -> String {
    match error {
        AppError::Validation(message) => format!("{action}：{message}"),
        _ => format!(
            "{action}（错误类型：{}）；详细信息已隐藏。",
            error.error_code()
        ),
    }
}
