//! RyFrame 的仓库级开发任务入口。
//!
//! 日常开发通过 `.cargo/config.toml` 中的短命令调用这里；`xtask` 仅保留为内部实现和
//! CI 的高级入口。每类任务放在独立模块中，避免命令解析、业务编排与进程管理互相耦合。

mod check;
mod cli;
mod contract;
mod dev;
#[cfg(feature = "resource")]
mod diff;
mod doctor;
mod migration;
mod process;
mod release;
mod resource;
mod watch;
mod workspace;

use std::{env, error::Error};

use cli::{Cli, Command};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() {
    let cli = match cli::parse(env::args().skip(1).collect()) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("参数错误：{error}");
            eprintln!("运行 `cargo xtask --help` 查看完整用法。");
            std::process::exit(2);
        }
    };

    if let Err(error) = dispatch(cli) {
        eprintln!("任务失败：{error}");
        std::process::exit(1);
    }
}

fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Doctor => doctor::run(&cli.frontend_dir),
        Command::Check { scope } => check::run(scope, &cli.frontend_dir),
        Command::Contract { operation } => contract::run(operation, &cli.frontend_dir),
        Command::FeatureMatrix => check::feature_matrix(),
        Command::ReleaseVerify(options) => release::verify(&options, &cli.frontend_dir),
        Command::Dev => dev::run(&cli.frontend_dir),
        Command::Verify { scope, full } => check::verify(scope, full, &cli.frontend_dir),
        Command::Resource(command) => resource::run(&command, &cli.frontend_dir),
        Command::ApiSync(command) => contract::api_sync(&command, &cli.frontend_dir),
        Command::Migrate(command) => migration::run(&command),
        Command::Help(topic) => {
            cli::print_help(topic.as_deref());
            Ok(())
        }
    }
}
