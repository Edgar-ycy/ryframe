use std::{path::PathBuf, time::Duration};

pub(crate) const RUNTIME_USAGE: &str = "用法：cargo xtask check recovery runtime <build|register|start|status|stop|recover|bind|verify> ...\n  build --source-backend <绝对目录> --expected-head <SHA> --source-frontend <绝对目录> --expected-frontend-head <SHA> --output <新收据> [--frontend-dir <工具前端绝对目录>] [--adapter-contract <合同> --product-backend <绝对目录>] --write\n  register --plan <参考计划> --target-plan <目标计划> --output <新登记> --write\n  start --runtime-registration <登记> --target-plan <目标计划> --source-backend <绝对目录> --source-frontend <绝对目录> --build-receipt <构建收据> --bindings <数据绑定> [--adapter-contract <合同> --product-backend <绝对目录>] [--timeout <秒>] --write\n  status --runtime-registration <登记> --target-plan <目标计划>\n  stop --runtime-registration <登记> --target-plan <目标计划> --generation <正整数> --write\n  recover --runtime-registration <登记> --target-plan <目标计划> --generation <正整数> --owner <控制文件> --write\n  bind --source-backend <绝对目录> --source-frontend <绝对目录> --build-receipt <构建收据> --launch-receipt <启动收据> --bindings <数据绑定> --output <新运行收据> [--adapter-contract <合同> --product-backend <绝对目录>] --write\n  verify --source-backend <绝对目录> --source-frontend <绝对目录> --bindings <数据绑定> --receipt <运行收据> [--adapter-contract <合同> --product-backend <绝对目录>]";

pub(crate) const LEGACY_B0_ADAPTER_CONTRACT: &str = "legacy-stable-readiness-b0-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeOperation {
    Build,
    Register,
    Start,
    Status,
    Stop,
    Recover,
    Bind,
    Verify,
}

impl RuntimeOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Register => "register",
            Self::Start => "start",
            Self::Status => "status",
            Self::Stop => "stop",
            Self::Recover => "recover",
            Self::Bind => "bind",
            Self::Verify => "verify",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeSources {
    pub(crate) source_backend: PathBuf,
    pub(crate) source_frontend: PathBuf,
    pub(crate) adapter_contract: Option<String>,
    pub(crate) product_backend: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeControlInputs {
    pub(crate) runtime_registration: PathBuf,
    pub(crate) target_plan: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeBuildOptions {
    pub(crate) sources: RuntimeSources,
    pub(crate) expected_head: String,
    pub(crate) expected_frontend_head: String,
    pub(crate) output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeRegistrationOptions {
    pub(crate) plan: PathBuf,
    pub(crate) target_plan: PathBuf,
    pub(crate) output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeStartOptions {
    pub(crate) control: RuntimeControlInputs,
    pub(crate) sources: RuntimeSources,
    pub(crate) build_receipt: PathBuf,
    pub(crate) bindings: PathBuf,
    pub(crate) timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeGenerationOptions {
    pub(crate) control: RuntimeControlInputs,
    pub(crate) generation: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeRecoverOptions {
    pub(crate) generation: RuntimeGenerationOptions,
    pub(crate) owner: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeBindOptions {
    pub(crate) sources: RuntimeSources,
    pub(crate) build_receipt: PathBuf,
    pub(crate) launch_receipt: PathBuf,
    pub(crate) bindings: PathBuf,
    pub(crate) output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeVerifyOptions {
    pub(crate) sources: RuntimeSources,
    pub(crate) bindings: PathBuf,
    pub(crate) receipt: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeCommand {
    Help(Option<RuntimeOperation>),
    Build(RuntimeBuildOptions),
    Register(RuntimeRegistrationOptions),
    Start(RuntimeStartOptions),
    Status(RuntimeControlInputs),
    Stop(RuntimeGenerationOptions),
    Recover(RuntimeRecoverOptions),
    Bind(RuntimeBindOptions),
    Verify(RuntimeVerifyOptions),
}

impl RuntimeCommand {
    pub(crate) const fn operation(&self) -> Option<RuntimeOperation> {
        match self {
            Self::Help(operation) => *operation,
            Self::Build(_) => Some(RuntimeOperation::Build),
            Self::Register(_) => Some(RuntimeOperation::Register),
            Self::Start(_) => Some(RuntimeOperation::Start),
            Self::Status(_) => Some(RuntimeOperation::Status),
            Self::Stop(_) => Some(RuntimeOperation::Stop),
            Self::Recover(_) => Some(RuntimeOperation::Recover),
            Self::Bind(_) => Some(RuntimeOperation::Bind),
            Self::Verify(_) => Some(RuntimeOperation::Verify),
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        matches!(
            self,
            Self::Build(_)
                | Self::Register(_)
                | Self::Start(_)
                | Self::Stop(_)
                | Self::Recover(_)
                | Self::Bind(_)
        )
    }
}
