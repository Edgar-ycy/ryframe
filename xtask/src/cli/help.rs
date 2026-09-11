pub(crate) fn print_help(topic: Option<&str>) {
    let help = match topic {
        Some("dev") => {
            "cargo xtask dev [--measure-once] [--frontend-dir PATH]\n  管理 API、Worker 与 Vite；只校验数据库迁移。--measure-once 仅供 DevEx 保存场景使用。"
        }
        Some("check") => check_help(),
        Some("build") => {
            "cargo xtask build [--profile release|dev] [--real] [--plan] [--frontend-dir PATH]\n  默认构建 release API、Worker 与前端生产产物；--real 追加真实前端构建收据；--plan 只输出同一构建计划，不执行或落盘。"
        }
        Some("generate") => generate_help(),
        Some("data") => data_help(),
        Some(unknown) => {
            eprintln!("未知帮助主题：{unknown}");
            general_help()
        }
        None => general_help(),
    };
    println!("{help}");
}

fn check_help() -> &'static str {
    CHECK_HELP
}

pub(crate) const CHECK_HELP: &str = "cargo xtask check [--full] [--plan] [--scope all|backend|frontend]\n  cargo xtask check doctor\n  cargo xtask check ci <plan|preflight|rust-gate|resource-gate|integration|consumer-contract>\n  cargo xtask check ci resource-gate replay ...\n  cargo xtask check perf run|paired|summarize|compare ...\n  cargo xtask check release ...\n  cargo xtask check recovery <plan|check-dataset|check-existing|dataset|backup|restore|copy|damage> ...\n  cargo xtask check recovery inputs <reference|product|bindings> ...\n  cargo xtask check recovery clone <阶段> ...\n  cargo xtask check recovery <runtime|source> <阶段> ...\n  cargo xtask check recovery fresh-target ...\n  cargo xtask check recovery fixture --output-dir <目录> --write\n  cargo xtask check recovery fixture <environment|review|request|successor|artifact|retention|services|source-pair|runtime|dataset> ...\n  cargo xtask check recovery fixture services <rustfs|redis|buckets|status|close|recover|restart> ...\n  cargo xtask check recovery dataset-prepare ...\n\n恢复入口固定当前后端工作树；需要前端的阶段使用 --frontend-dir。默认根据变更执行最小安全检查；--full 执行浏览器以外的完整本地门禁；--plan 只输出任务图。";

fn generate_help() -> &'static str {
    "cargo xtask generate resource <资源名> [--check|--write|--explain]\n  cargo xtask generate resource --all <--check|--write>\n  cargo xtask generate api [--write]\n  cargo xtask generate api --commit <SHA> --write\n\n未传 --write 时只预览或核验，不更新文件。--all --write 在一个事务中更新相互关联的资源资产。"
}

fn data_help() -> &'static str {
    "cargo xtask data migrate ...\n  cargo xtask data backup <inventory|register|status> ...\n  cargo xtask data restore <begin|verify-data|verify> ...\n  cargo xtask data target inventory ...\n  cargo xtask data file <backfill-sha256|drain-legacy-reservations> ...\n  cargo xtask data reset <plan|execute> ...\n\n所有数据写入都必须继续满足底层命令的显式确认、计划和 ownership 要求。"
}

fn general_help() -> &'static str {
    "RyFrame 开发命令\n\n  cargo xtask dev\n  cargo xtask check [--full]\n  cargo xtask build\n  cargo xtask generate <resource|api> ...\n  cargo xtask data <migrate|backup|restore|target|file|reset> ...\n\n运行 `cargo xtask <命令> --help` 查看参数。"
}
