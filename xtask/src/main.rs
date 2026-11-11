//! RyFrame 的仓库级开发任务入口。
//!
//! 所有公开自动化都从 `cargo xtask` 进入。每类任务放在独立模块中，避免命令解析、
//! 业务编排与进程管理互相耦合。

mod build;
mod build_source;
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
mod frontend_origin;
mod http_session;
mod local_test_path;
mod login_budget;
mod migration;
mod process;
mod rate_limit;
mod recovery;
mod release;
mod resource;
mod session_pacing;
mod source_edit;
mod source_scan;
mod watch;
mod workspace;

use std::{env, error::Error};

use cli::{CheckCommand, Cli, Command, DataCommand, GenerateCommand};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() {
    if run_hidden_handlers() {
        return;
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
        Command::Dev(options) => dev::execute(&options),
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
        CheckCommand::Language(options) => check::language::run(&options),
        CheckCommand::Doctor => doctor::run(frontend_dir),
        CheckCommand::Ci(command) => ci::run(command, frontend_dir),
        CheckCommand::Perf(command) => devex::run(&command, frontend_dir),
        CheckCommand::Release(command) => release::run(&command, frontend_dir),
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

fn run_hidden_handlers() -> bool {
    run_recovery_hidden_handlers();
    #[cfg(target_os = "linux")]
    if let Some(result) = devex::memory::run_trampoline_if_requested() {
        if let Err(error) = result {
            eprintln!("DevEx cgroup 执行跳板失败：{error}");
            std::process::exit(125);
        }
        unreachable!("成功的 exec 不会返回");
    }
    if let Some(code) = devex::runtime_native::entry::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = recovery::clone::native::s3::protocol::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = recovery::clone::native::session::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = recovery::frontend_build::protocol::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = frontend_origin::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = devex::job_evidence::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = recovery::monitoring::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = rate_limit::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = devex::import_samples::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(result) = devex::cgroup::run_hidden_if_requested() {
        if let Err(error) = result {
            eprintln!("DevEx cgroup 内部阶段失败：{error}");
            std::process::exit(125);
        }
        return true;
    }

    false
}

fn run_recovery_hidden_handlers() {
    if let Some(code) = recovery::run_clone_exec_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) =
        recovery::runtime::dataset_authority::verify_existing::run_hidden_if_requested()
    {
        std::process::exit(code);
    }
    if let Some(code) = recovery::runtime::frontend_site::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = recovery::runtime::start::actor::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = recovery::full_stack::worker::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = recovery::fixture_control::environment::protocol::run_hidden_if_requested()
    {
        std::process::exit(code);
    }
    if let Some(code) = recovery::clone::native::cache::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = recovery::clone::native::post_process::protocol::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = recovery::fixture_runtime::protocol::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) =
        recovery::fixture_control::services::lifecycle::protocol::run_hidden_if_requested()
    {
        std::process::exit(code);
    }
    if let Some(code) = ci::databases::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = recovery::source::evidence::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) =
        recovery::fixture_control::services::history::protocol::run_hidden_if_requested()
    {
        std::process::exit(code);
    }
    if let Some(code) =
        recovery::clone::native::target_verify::evidence_protocol::run_hidden_if_requested()
    {
        std::process::exit(code);
    }
    if let Some(code) = recovery::clone::native::seed::protocol::run_hidden_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = recovery::dataset_prepare::run_hidden_if_requested() {
        std::process::exit(code);
    }
}
