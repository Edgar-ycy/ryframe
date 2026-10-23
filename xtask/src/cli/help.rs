pub(crate) fn print_help(topic: Option<&str>) {
    let help = match topic {
        Some("dev") => {
            "cargo xtask dev [--config-dir <目录>] [--workspace-root <后端根>] [--plan] [--measure-once [--case <场景>] [--output <文件>]]\n  只管理后端 API、Worker 与其进程树；只校验数据库迁移。前端请在前端仓库运行 corepack pnpm dev。配置目录优先级：CLI、本机 tools、公共 tools、workspace/config；相对 workspace 解析。工作区根仅由显式参数或 cwd 向上寻找，不读取旧环境入口。--plan 不执行 IO。--case 与 --output 仅用于 --measure-once，场景默认 shared-runtime。DevEx 保存场景：config-only/api-only/worker-only/shared-runtime/locales/migration-only/resource-manifest/cancellation。"
        }
        Some("check") => {
            println!(
                "{CHECK_HELP}\n\n{CHECK_REPLAY_HELP}\n\n{CGROUP_HELP}\n\n{CI_INTEGRATION_HELP}\n\n{SUPPLY_CHAIN_HELP}\n\n{RELEASE_HELP}"
            );
            return;
        }
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

pub(crate) const CI_INTEGRATION_HELP: &str = "ci integration 使用 Rust xtask 生成临时回环 CA，并启动 Redis TLS 代理与 HTTPS fixture；需要本机 OpenSSL 和回环 Redis，不调用 Python。先配置 .local-tests/integration/ci.toml 的 format_version、run_id、artifact_root、redis_config_dir、openssl_program，可选 test_jobs、timeout_secs；连接与凭据读取 redis_config_dir 下 APP_ENV 选中的 app 配置。Redis host 只接受 127.0.0.1、::1 或 localhost；openssl_program 使用明确值 openssl，或普通无链接程序的绝对路径、以后端根目录解析且无父目录跳转的相对路径。不读取旧环境覆盖；产物在 artifact_root 下排他创建，失败保留诊断与未确认清理现场。";

pub(crate) const CGROUP_HELP: &str = "cgroup run 只允许 Linux GitHub Actions 的非 root runner，证据目录必须新创建；Rust xtask 内部按 prepare、test、cleanup 权限边界执行，Cargo 在独立降权进程中启动。cleanup 只读取同一 run/attempt 的创建与进程登记收据并精确冻结、终止和删除本次 cgroup。--plan 只输出同一任务图，不启动 sudo、不创建目录。";

pub(crate) const CHECK_HELP: &str = "cargo xtask check [--full] [--plan] [--scope all|backend|frontend]\n  cargo xtask check language [--min-rust-percent <0..100>] [--output <.local-tests 内新 JSON> --write] [--plan]\n  cargo xtask check doctor\n  cargo xtask check ci <plan|preflight|rust-gate|resource-gate|integration|consumer-contract|frontend-source|required|security>\n  cargo xtask check ci <plan|preflight> [--base-sha <SHA>]\n  cargo xtask check ci resource-gate [--base-sha <SHA>] [--head-sha <SHA>] [--frontend-ref <SHA>] [--targeted] [--decision-file <绝对文件>] [--jobs <1..64>] [--test-jobs <1..64>]\n  cargo xtask check ci rust-gate [--frontend-ref <SHA>] [--profile standard|windows-smoke] [--test-jobs <1..64>]\n  cargo xtask check ci consumer-contract [--frontend-ref <SHA>] [--backend-head <SHA> --backend-repository <owner/repo> --candidate-openapi <绝对文件>]\n  cargo xtask check ci frontend-source --event-name <事件> [--event <绝对文件>] [--base-sha <SHA>] [--prefer-marker] [--candidate-openapi <绝对文件>] [--release-ref <ref>] [--fallback-main-on-invalid-base] [--plan]\n  cargo xtask check ci security source\n  cargo xtask check ci security report <cyclonedx|trivy> --input <绝对文件> [--require-reproducible]\n  cargo xtask check ci security deployment <source|image> ...\n  cargo xtask check ci resource-gate replay ...\n  cargo xtask check ci required --event <push|pull_request> [--action <动作>] --needs-json <JSON>\n  cargo xtask check perf run|paired|summarize|compare ...\n  cargo xtask check perf cgroup <run|cleanup> --output <.local-tests 内目录> [--plan]\n  cargo xtask check release source --tag <tag> --backend-repository <owner/repo> --backend-commit <SHA> --frontend-repository <owner/repo> --frontend-commit <SHA> --manifest-path <绝对文件> [--frontend-dir <绝对目录>] [--plan]\n  cargo xtask check release ci --backend-repository <owner/repo> --frontend-repository <owner/repo> --backend-sha <SHA> --frontend-sha <SHA> [--backend-tag-oid <SHA> --frontend-tag-oid <SHA>] --tag <tag> --timeout <秒> --output <绝对文件> [--plan]\n  cargo xtask check release ci record-pair --output <绝对文件> [--frontend-dir <绝对目录>] [--plan]\n  cargo xtask check release ci verify-pair --input <绝对文件> [--frontend-dir <绝对目录>] [--plan]\n  cargo xtask check recovery <plan|check-dataset|check-existing|dataset|backup|restore|copy|damage> ...\n  cargo xtask check recovery inputs <reference|product|bindings> ...\n  cargo xtask check recovery clone <阶段> ...\n  cargo xtask check recovery <runtime|source> <阶段> ...\n  cargo xtask check recovery fresh-target ...\n  cargo xtask check recovery fixture --output-dir <目录> [--expected-backend-sha <SHA> --expected-frontend-sha <SHA>] --write\n  cargo xtask check recovery fixture <environment|review|request|successor|artifact|retention|services|source-pair|runtime|dataset> ...\n  cargo xtask check recovery fixture runtime --help\n  cargo xtask check recovery fixture services <rustfs|redis|buckets|status|close|reconcile|recover|restart> ...\n  cargo xtask check recovery full-stack <prepare|rate-limit|start|collect> ...\n  cargo xtask check recovery monitoring <bind|start|observe|close|result|status> ...\n  cargo xtask check recovery dataset-prepare ...\n\nlanguage 默认仅向 stdout 报告固定口径的原始文件字节；包含测试、生成代码和模板，vendor 单列。显式 --min-rust-percent 才校验阈值，未达或代码分母为空时失败。--output 必须同时指定 --write，仅创建忽略目录中的新文件，不创建父目录。--plan 不执行 Git 或落盘。\n\n恢复入口固定当前后端工作树；需要前端的阶段使用 --frontend-dir。发布核验的显式路径必须是绝对路径。默认只规划后端检查，不要求前端；--scope all 显式执行跨仓检查。--full 执行所选范围的完整本地门禁（不含真实浏览器）；--plan 只输出同一任务图。前端路径优先级：--frontend-dir、.local-tests/tools.toml、config/tools.toml，均以后端根目录解析。\n\npython.environment 由 Rust 严格解析 scripts/requirements-ci.txt，并以一次固定 stdin/UTF-8 协议核验解释器版本和发行版版本，再执行 tools/python/rust_function_size.py 的 64 轮 --native-probe。Python 由 config/tools.toml 的 verification.python_program 选择，.local-tests/tools.toml 中同名字段优先覆盖；未配置时从操作系统 PATH 选择并核验解释器；--plan 不启动 Python。";

pub(crate) const CHECK_REPLAY_HELP: &str = "资源门禁回放入口由 Rust xtask 原生执行：\n  cargo xtask check ci resource-gate replay --manifest <文件> --work-dir <.local-tests/resource-gate-replay 内目录> --report <同一目录内新文件> [--activation-gate]\n  manifest 的 targeted/full 命令共用同一计划，report 拒绝覆盖；回放会为两个仓库创建隔离 worktree，复用会话级 target 与专用 sccache，并在退出前停止服务、删除登记目录。\n  --activation-gate 额外要求隔离 MySQL、Redis、TLS 环境和固定任务数，核验 20 个以上案例、至少 10 个成功 targeted 案例、零分歧、缓存健康及 P95 预算。";

const SUPPLY_CHAIN_HELP: &str = "security source 的供应链策略、工作流固定引用、本地 patch 许可证和 Cargo feature 树由 Rust 只读校验，不启动 Python。";

const RELEASE_HELP: &str = "发布核验直接由 Rust xtask 执行，不依赖 Python。";

fn generate_help() -> &'static str {
    "cargo xtask generate resource <资源名> [--check|--write|--explain]\n  cargo xtask generate resource --all <--check|--write>\n  cargo xtask generate api [--write]\n  cargo xtask generate api --commit <SHA> --write\n\n未传 --write 时只预览或核验，不更新文件。--all --write 在一个事务中更新相互关联的资源资产。"
}

fn data_help() -> &'static str {
    "cargo xtask data migrate check [--require-frozen] [--trusted-ref <ref-or-sha>]\n  cargo xtask data migrate baseline --write\n  cargo xtask data migrate freeze\n  cargo xtask data migrate reconcile --write\n\n  cargo xtask data performance-identities plan --environment <绝对环境清单> --output <绝对计划> --write\n  cargo xtask data performance-identities <apply|verify> --plan <绝对计划> --state-dir <绝对账本目录> --write\n  cargo xtask data backup inventory --output <绝对新文件> --source-sha <SHA> <--quiesced-at|--observed-at> <RFC3339>\n  cargo xtask data backup register --manifest <绝对文件> --backup-root <绝对目录> --write\n  cargo xtask data backup status\n  cargo xtask data restore begin --plan <绝对文件> --output <绝对新文件> --restore-config-dir <绝对目录> --write\n  cargo xtask data restore verify-data --id <ID> --backup-root <绝对目录> --output <绝对新文件> --restore-config-dir <绝对目录> --write\n  cargo xtask data restore verify --id <ID> --proof <绝对文件> --tests-receipt <绝对文件> --runtime-receipt <绝对文件> --target-plan <绝对文件> --runner-root <绝对目录> --restore-config-dir <绝对目录> --write\n  cargo xtask data target inventory --target <目标键> --output <绝对新文件>\n  cargo xtask data reset plan\n  cargo xtask data reset execute --plan-hash <sha256> --confirm-reset <精确短语> --write\n\n迁移 check 默认与 --require-frozen 只读；baseline --write 仅用于 0.x 显式重整基线，reconcile --write 按版本阶段选择 baseline 或 freeze。性能身份、证据输入与证据输出必须位于当前后端 .local-tests；备份根、隔离配置和 runner 可使用已登记的外部绝对目录。所有路径都拒绝链接。backup inventory 与 target inventory 是显式选择的只读库存操作，只在 .local-tests 原子创建新证据文件，不写业务资源，因此不使用 --write；状态读取同样不使用 --write。发布登记记录、性能身份准备或修改业务资源必须显式 --write，并继续满足底层 ownership、计划及确认要求。"
}

fn general_help() -> &'static str {
    "RyFrame 开发命令\n\n  cargo xtask dev\n  cargo xtask check [--full]\n  cargo xtask build\n  cargo xtask generate <resource|api> ...\n  cargo xtask data <migrate|performance-identities|backup|restore|target|reset> ...\n\n运行 `cargo xtask <命令> --help` 查看参数。"
}
