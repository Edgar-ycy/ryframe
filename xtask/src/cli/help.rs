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

pub(crate) const CHECK_HELP: &str = "cargo xtask check [--full] [--plan] [--scope all|backend|frontend]\n  cargo xtask check doctor\n  cargo xtask check ci <plan|preflight|rust-gate|resource-gate|integration|consumer-contract|frontend-source|required|security>\n  cargo xtask check ci frontend-source --event-name <事件> [--event <绝对文件>] [--base-sha <SHA>] [--prefer-marker] [--candidate-openapi <绝对文件>] [--release-ref <ref>] [--fallback-main-on-invalid-base] [--plan]\n  cargo xtask check ci security source\n  cargo xtask check ci security report <cyclonedx|trivy> --input <绝对文件> [--require-reproducible]\n  cargo xtask check ci security deployment <source|image> ...\n  cargo xtask check ci resource-gate replay ...\n  cargo xtask check ci required --event <push|pull_request> [--action <动作>] --needs-json <JSON>\n  cargo xtask check perf run|paired|summarize|compare ...\n  cargo xtask check perf cgroup <run|cleanup> --output <.local-tests 内目录> [--plan]\n  cargo xtask check release source --tag <tag> --backend-repository <owner/repo> --backend-commit <SHA> --frontend-repository <owner/repo> --frontend-commit <SHA> --manifest-path <绝对文件> [--frontend-dir <绝对目录>] [--plan]\n  cargo xtask check release ci --backend-repository <owner/repo> --frontend-repository <owner/repo> --backend-sha <SHA> --frontend-sha <SHA> [--backend-tag-oid <SHA> --frontend-tag-oid <SHA>] --tag <tag> --timeout <秒> --output <绝对文件> [--plan]\n  cargo xtask check release ci record-pair --output <绝对文件> [--frontend-dir <绝对目录>] [--plan]\n  cargo xtask check release ci verify-pair --input <绝对文件> [--frontend-dir <绝对目录>] [--plan]\n  cargo xtask check recovery <plan|check-dataset|check-existing|dataset|backup|restore|copy|damage> ...\n  cargo xtask check recovery inputs <reference|product|bindings> ...\n  cargo xtask check recovery clone <阶段> ...\n  cargo xtask check recovery <runtime|source> <阶段> ...\n  cargo xtask check recovery fresh-target ...\n  cargo xtask check recovery fixture --output-dir <目录> [--expected-backend-sha <SHA> --expected-frontend-sha <SHA>] --write\n  cargo xtask check recovery fixture <environment|review|request|successor|artifact|retention|services|source-pair|runtime|dataset> ...\n  cargo xtask check recovery fixture runtime --help\n  cargo xtask check recovery fixture services <rustfs|redis|buckets|status|close|recover|restart> ...\n  cargo xtask check recovery full-stack <prepare|rate-limit|start|collect> ...\n  cargo xtask check recovery monitoring <bind|start|observe|close|result|status> ...\n  cargo xtask check recovery dataset-prepare ...\n\n恢复入口固定当前后端工作树；需要前端的阶段使用 --frontend-dir。发布核验的显式路径必须是绝对路径。默认根据变更执行最小安全检查；--full 执行浏览器以外的完整本地门禁；--plan 只输出任务图。";

fn generate_help() -> &'static str {
    "cargo xtask generate resource <资源名> [--check|--write|--explain]\n  cargo xtask generate resource --all <--check|--write>\n  cargo xtask generate api [--write]\n  cargo xtask generate api --commit <SHA> --write\n\n未传 --write 时只预览或核验，不更新文件。--all --write 在一个事务中更新相互关联的资源资产。"
}

fn data_help() -> &'static str {
    "cargo xtask data migrate ...\n  cargo xtask data performance-identities plan --environment <绝对环境清单> --output <绝对计划> --write\n  cargo xtask data performance-identities <apply|verify> --plan <绝对计划> --state-dir <绝对账本目录> --write\n  cargo xtask data backup inventory --output <绝对新文件> --source-sha <SHA> <--quiesced-at|--observed-at> <RFC3339>\n  cargo xtask data backup register --manifest <绝对文件> --backup-root <绝对目录> --write\n  cargo xtask data backup status\n  cargo xtask data restore begin --plan <绝对文件> --output <绝对新文件> --restore-config-dir <绝对目录> --write\n  cargo xtask data restore verify-data --id <ID> --backup-root <绝对目录> --output <绝对新文件> --restore-config-dir <绝对目录> --write\n  cargo xtask data restore verify --id <ID> --proof <绝对文件> --tests-receipt <绝对文件> --runtime-receipt <绝对文件> --target-plan <绝对文件> --runner-root <绝对目录> --restore-config-dir <绝对目录> --write\n  cargo xtask data target inventory --target <目标键> --output <绝对新文件>\n  cargo xtask data file <backfill-sha256|drain-legacy-reservations> <dry-run|apply> --database <名称> [--batch-size <1..1000>] [--start-after <ID>] [--write --confirm-apply APPLY-FILE-A-MAINTENANCE]\n  cargo xtask data reset plan\n  cargo xtask data reset execute --plan-hash <sha256> --confirm-reset <精确短语> --write\n\n性能身份、证据输入与证据输出必须位于当前后端 .local-tests；备份根、隔离配置和 runner 可使用已登记的外部绝对目录。所有路径都拒绝链接。backup inventory 与 target inventory 是显式选择的只读库存操作，只在 .local-tests 原子创建新证据文件，不写业务资源，因此不使用 --write；状态读取同样不使用 --write。发布登记记录、性能身份准备或修改业务资源必须显式 --write，并继续满足底层 ownership、计划及确认要求。"
}

fn general_help() -> &'static str {
    "RyFrame 开发命令\n\n  cargo xtask dev\n  cargo xtask check [--full]\n  cargo xtask build\n  cargo xtask generate <resource|api> ...\n  cargo xtask data <migrate|performance-identities|backup|restore|target|file|reset> ...\n\n运行 `cargo xtask <命令> --help` 查看参数。"
}
