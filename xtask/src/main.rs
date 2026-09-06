//! RyFrame 的仓库级开发任务入口。
//!
//! 所有公开自动化都从 `cargo xtask` 进入。每类任务放在独立模块中，避免命令解析、
//! 业务编排与进程管理互相耦合。

mod build;
mod check;
mod ci;
mod cli;
mod contract;
mod data;
mod dev;
mod devex;
#[cfg(feature = "resource")]
mod diff;
mod doctor;
mod migration;
mod process;
mod recovery;
mod release;
mod resource;
mod source_edit;
mod watch;
mod workspace;

use std::{env, error::Error};

use cli::{CheckCommand, Cli, Command, DataCommand, GenerateCommand};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() {
    #[cfg(target_os = "linux")]
    if let Some(result) = devex::memory::run_trampoline_if_requested() {
        if let Err(error) = result {
            eprintln!("DevEx cgroup 执行跳板失败：{error}");
            std::process::exit(125);
        }
        unreachable!("成功的 exec 不会返回");
    }

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
        let code = dev::failure_exit_code(error.as_ref())
            .or_else(|| process::failure_exit_code(error.as_ref()))
            .unwrap_or(1);
        std::process::exit(code);
    }
}

fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Dev { measure_once: true } => dev::measure_once(),
        Command::Dev {
            measure_once: false,
        } => dev::run(&cli.frontend_dir),
        Command::Check(command) => dispatch_check(command, &cli.frontend_dir),
        Command::Build(options) => build::run(options, &cli.frontend_dir),
        Command::Generate(command) => dispatch_generate(command, &cli.frontend_dir),
        Command::Data(command) => dispatch_data(command),
        Command::Help(topic) => {
            cli::print_help(topic.as_deref());
            Ok(())
        }
    }
}

fn dispatch_check(command: CheckCommand, frontend_dir: &std::path::Path) -> Result<()> {
    match command {
        CheckCommand::Run(options) if options.plan => {
            check::plan(options.scope, options.full, frontend_dir)
        }
        CheckCommand::Run(options) => check::verify(options.scope, options.full, frontend_dir),
        CheckCommand::Doctor => doctor::run(frontend_dir),
        CheckCommand::Ci(command) => ci::run(command, frontend_dir),
        CheckCommand::Perf(command) => devex::run(&command, frontend_dir),
        CheckCommand::Release(options) => release::verify(&options, frontend_dir),
        CheckCommand::Recovery(command) => recovery::run(&command, frontend_dir),
    }
}

fn dispatch_generate(command: GenerateCommand, frontend_dir: &std::path::Path) -> Result<()> {
    match command {
        GenerateCommand::Help => {
            cli::print_help(Some("generate"));
            Ok(())
        }
        GenerateCommand::Resource(command) => resource::run(&command, frontend_dir),
        GenerateCommand::Api(command) => contract::generate_api(&command, frontend_dir),
    }
}

fn dispatch_data(command: DataCommand) -> Result<()> {
    match command {
        DataCommand::Help => {
            cli::print_help(Some("data"));
            Ok(())
        }
        DataCommand::Migrate(command) => migration::run(&command),
        command => data::run(&command),
    }
}
