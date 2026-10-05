use std::path::PathBuf;

pub(crate) const FIXTURE_RUNTIME_USAGE: &str = "用法：cargo xtask check recovery fixture runtime <build|verify|start|stop|status> --environment <绝对 bootstrap.json> --output <绝对运行目录> [--write]\n  cargo xtask check recovery fixture runtime bind --environment <绝对 bootstrap.json> --output <绝对运行目录> --browser-binding <绝对新文件> --run-id <标识> --server <dev|preview> --write\n  cargo xtask check recovery fixture runtime <browser|browser-verify|browser-close> --environment <绝对 bootstrap.json> --output <绝对运行目录> --browser-binding <绝对绑定文件> [--write]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FixtureServer {
    Dev,
    Preview,
}

impl FixtureServer {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Preview => "preview",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureRuntimePaths {
    pub(crate) environment: PathBuf,
    pub(crate) output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureBrowserBinding {
    pub(crate) runtime: FixtureRuntimePaths,
    pub(crate) browser_binding: PathBuf,
}

/// 业务 crate 夹具运行时的公开请求。路径和写入语义在构造此类型前已经核验。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FixtureRuntimeCommand {
    Help,
    Build(FixtureRuntimePaths),
    Verify(FixtureRuntimePaths),
    Start(FixtureRuntimePaths),
    Stop(FixtureRuntimePaths),
    Status(FixtureRuntimePaths),
    Bind {
        binding: FixtureBrowserBinding,
        run_id: String,
        server: FixtureServer,
    },
    Browser(FixtureBrowserBinding),
    BrowserVerify(FixtureBrowserBinding),
    BrowserClose(FixtureBrowserBinding),
}

impl FixtureRuntimeCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Build(_) => "build",
            Self::Verify(_) => "verify",
            Self::Start(_) => "start",
            Self::Stop(_) => "stop",
            Self::Status(_) => "status",
            Self::Bind { .. } => "bind",
            Self::Browser(_) => "browser",
            Self::BrowserVerify(_) => "browser-verify",
            Self::BrowserClose(_) => "browser-close",
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        matches!(
            self,
            Self::Build(_) | Self::Start(_) | Self::Stop(_) | Self::Bind { .. } | Self::Browser(_)
        )
    }

    pub(crate) const fn runtime(&self) -> Option<&FixtureRuntimePaths> {
        match self {
            Self::Help => None,
            Self::Build(paths)
            | Self::Verify(paths)
            | Self::Start(paths)
            | Self::Stop(paths)
            | Self::Status(paths) => Some(paths),
            Self::Bind { binding, .. }
            | Self::Browser(binding)
            | Self::BrowserVerify(binding)
            | Self::BrowserClose(binding) => Some(&binding.runtime),
        }
    }

    pub(crate) const fn browser_binding(&self) -> Option<&PathBuf> {
        match self {
            Self::Bind { binding, .. }
            | Self::Browser(binding)
            | Self::BrowserVerify(binding)
            | Self::BrowserClose(binding) => Some(&binding.browser_binding),
            _ => None,
        }
    }
}
