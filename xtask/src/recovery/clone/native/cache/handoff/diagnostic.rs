//! 交接阶段只公开固定原因、系统码和原收据位置，原始错误保留在原因链中。
use crate::Result;
use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy)]
pub(super) enum Phase {
    Binding,
    BeforeStart,
    Start,
    AfterStart,
    Cleanup,
    Observe,
    BeforeStop,
    Stop,
    AfterStop,
    Recover,
}

impl Phase {
    fn details(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Observe => (
                "ApiHandoffObservationRejected",
                "只读完整树观察",
                "核对原API/Worker成员、创建身份、监听及准备合同；本次不发送启动或停止",
            ),
            Self::BeforeStop => (
                "ApiHandoffPreStopRejected",
                "停止前核验",
                "核对原完整树、生产者历史、冻结环境及控制锁；本次尚未调用停止",
            ),
            Self::Stop => (
                "ApiHandoffStopUnconfirmed",
                "原生监督停止",
                "核对原api-control.lock、停止结果与全部成员关闭证明；不能补发stop",
            ),
            Self::AfterStop => (
                "ApiHandoffPostStopRejected",
                "停止后核验",
                "核对原生停止结果、全部成员和端口；历史发布失败不能推翻原生现场或重放stop",
            ),
            Self::Recover => (
                "ApiHandoffRecoveryRejected",
                "原外层锁回收",
                "核对worker-control.lock原owner、目录身份和完整树；角色reconcile不能代替原外层锁回收",
            ),
            Self::Binding => (
                "ApiHandoffBindingRejected",
                "输入与资源锁",
                "核对 post-copy.json、prepare.json、原复制账本及固定文件绑定",
            ),
            Self::BeforeStart => (
                "ApiHandoffPreStartRejected",
                "启动前核验",
                "核对目标物理身份、存储代次、原 owner 与 API/Worker 关闭证明",
            ),
            Self::Start => (
                "ApiHandoffStartUnconfirmed",
                "原生监督启动",
                "核对 api-control.lock、原操作结果和完整监督树；不能再次发送 start",
            ),
            Self::AfterStart => (
                "ApiHandoffPostStartRejected",
                "启动后核验",
                "核对 API 身份、实际参数、监听归属、来源和目标后像",
            ),
            Self::Cleanup => (
                "ApiHandoffCleanupRejected",
                "交接收尾",
                "核对原运行锁、目标锁、owner 与完整前后像；不能抢锁或补发停止",
            ),
        }
    }
}

struct Failure {
    phase: Phase,
    cause: Box<dyn Error>,
    secondary: Option<Box<dyn Error>>,
    location: Option<(PathBuf, u64)>,
}

impl fmt::Debug for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (category, phase, guidance) = self.phase.details();
        write!(
            formatter,
            "API交接 {category}；阶段：{phase}；原因：{}；{guidance}",
            reason(self.cause.as_ref())
        )?;
        match self.phase {
            Phase::AfterStart => {
                write!(formatter, "；已取得API启动成功收据，当前运行状态仍须核对")?
            }
            Phase::Start | Phase::Cleanup => {
                write!(formatter, "；API可能已经启动，结果须按原操作核对")?
            }
            Phase::Binding | Phase::BeforeStart => write!(formatter, "；本次尚未调用API启动")?,
            Phase::Observe | Phase::BeforeStop | Phase::Recover => {
                write!(formatter, "；本操作未发送API启动或停止")?
            }
            Phase::Stop | Phase::AfterStop => {
                write!(formatter, "；API可能已经停止，以原生结果与完整成员证明为准")?
            }
        }
        if self.has_secondary() {
            write!(formatter, "；锁或后置核验同时失败，保留两条原始原因")?;
        }
        if let Some((run, number)) = &self.location {
            write!(
                formatter,
                "；原失败收据：{}；阶段状态：{}",
                run.join(format!("failure-{number:04}.json")).display(),
                run.join("state.json").display()
            )?;
            write!(
                formatter,
                "；下一步：cargo xtask check recovery clone status --run-dir '{}'",
                run.display().to_string().replace('\'', "''")
            )?;
        }
        write!(
            formatter,
            "；按该运行登记的原 runtime 目录继续只读核验，保留现场，不自动重放"
        )
    }
}

impl Failure {
    fn has_secondary(&self) -> bool {
        self.secondary.is_some()
            || self
                .cause
                .downcast_ref::<Self>()
                .is_some_and(Self::has_secondary)
    }
}

impl Error for Failure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

pub(super) fn failure(
    phase: Phase,
    cause: Box<dyn Error>,
    secondary: Option<Box<dyn Error>>,
) -> Box<dyn Error> {
    let (phase, location) = cause
        .downcast_ref::<Failure>()
        .map(|error| (error.phase, error.location.clone()))
        .unwrap_or((phase, None));
    Box::new(Failure {
        phase,
        cause,
        secondary,
        location,
    })
}

pub(super) fn context<T>(
    run: &Path,
    attempt: Option<u64>,
    phase: Phase,
    result: Result<T>,
) -> Result<T> {
    result.map_err(|cause| {
        let mut wrapped = failure(phase, cause, None);
        if let Some(error) = wrapped.downcast_mut::<Failure>() {
            error.location = attempt
                .filter(|number| *number > 0)
                .map(|number| (run.to_owned(), number));
        }
        wrapped
    })
}

fn reason(error: &(dyn Error + 'static)) -> String {
    if let Some(error) = error.downcast_ref::<crate::recovery::clone::native::seed::source_registration::quiet::CimUnavailable>() {
        return error.to_string();
    }
    if let Some((_, message)) = crate::recovery::clone::native::target_verify::diagnostic(error) {
        return message;
    }
    if let Some(io) = error.downcast_ref::<std::io::Error>() {
        let description = match io.kind() {
            std::io::ErrorKind::PermissionDenied => {
                "操作系统拒绝访问，核对本机身份、文件和进程查询权限"
            }
            std::io::ErrorKind::NotFound => "所需文件或进程不存在，核对原绑定而不是创建替代输入",
            std::io::ErrorKind::AlreadyExists => "目标已存在，核对原操作及 owner，不覆盖或重放",
            std::io::ErrorKind::TimedOut => "操作系统等待超时，保留原期限与失败现场",
            _ => "操作系统 IO 失败，核对原收据与资源状态",
        };
        return format!("{description}；系统码：{:?}", io.raw_os_error());
    }
    let message = error.to_string();
    if let Some(reason) = recovery_reason(&message) {
        return reason.into();
    }
    match message.as_str() {
        "角色结果仍未知或失败；仅保留原生证据，不补发start或stop" => {
            "角色结果仍未知或失败；仅保留原生证据，不补发start或stop".into()
        }
        "角色尚有未完成控制证据；保留原操作并显式核对，不追加停止" => {
            "角色尚有未完成控制证据；保留原操作并显式核对，不追加停止".into()
        }
        "控制器仍在运行，禁止释放运行控制锁" => {
            "原外层控制器仍在运行，不能释放其运行锁".into()
        }
        "API生产者历史未形成完整已启动代次，保留未知现场" => {
            "API生产者历史未形成完整已启动代次，保留未知现场，不补签started或重放停止".into()
        }
        "API交接活动登记发生变化，不能回退旧成功" => {
            "API交接活动登记发生变化，不能回退旧成功".into()
        }
        "API交接控制器、完整账本或固定清单变化" => {
            "API交接控制器、完整账本或固定清单变化".into()
        }
        "API交接初始化、完整计划或复制后像变化" => {
            "API交接初始化、完整计划或复制后像变化".into()
        }
        "参考配置目录必须是项目config或后端受管.local-tests目录" => {
            "配置目录不属于已登记配置归属根".into()
        }
        "参考配置文件缺失或经过链接" => "已绑定配置文件缺失或经过链接".into(),
        "交接API物理目标不同于原复制目标" => "交接API物理目标不同于原复制目标".into(),
        "API完整实际参数不同于冻结产物和明确配置目录" => {
            "API完整实际参数不同于冻结产物和明确配置目录".into()
        }
        "API生产者历史与当前运行目录不一致，拒绝覆盖" => {
            "API生产者历史与当前运行目录不一致，拒绝覆盖".into()
        }
        "未登记生产者、旧运行目录或控制锁仍存在；保留现场，不自动接管" => {
            "旧运行目录、生产者或控制锁仍存在，不能自动接管".into()
        }
        _ => error.source().map(reason).unwrap_or_else(|| {
            "未分类的交接拒绝；核对本阶段列出的绑定与原始收据，不回显私有错误文本".into()
        }),
    }
}

fn recovery_reason(message: &str) -> Option<&'static str> {
    match message {
        "原API外层控制器必须为对象"
        | "原API外层控制器字段缺失或包含未知字段"
        | "原外层控制器owner形状或登记运行目录不符，不能回收" => {
            Some("原外层控制器 owner 六字段或格式不符；核对原登记，不创建替代 owner")
        }
        "原外层控制器owner运行目录必须是文本绝对路径" => {
            Some("原外层控制器 owner 的运行目录类型不符；必须保留原登记的文本绝对路径")
        }
        "原owner登记运行目录无效：路径必须是绝对路径"
        | "原owner登记运行目录无效：路径必须位于当前后端 .local-tests 的子目录"
        | "原owner登记运行目录无效：路径不能是 .local-tests 根目录"
        | "原owner登记运行目录无效：路径不得为空或包含父目录、当前目录跳转" => {
            Some("原外层控制器登记目录不满足受管绝对路径边界；不放宽边界或改写原 owner")
        }
        "原外层控制器登记运行目录不是同一受管真实目录" => {
            Some("原外层控制器登记运行目录与当前运行目录不符；两者必须是同一受管真实目录")
        }
        "原外层控制器登记运行目录身份在核对期间变化" => {
            Some("原外层控制器登记运行目录身份在核对期间变化；保留原锁与恢复证据")
        }
        "API外层控制锁目录身份、原owner或文件集合变化，保留现场" => {
            Some("原外层锁目录身份、owner 字节绑定或单 owner 文件集合发生变化；拒绝删除")
        }
        "外层锁owner进程观察在恢复发布期间变化，保留现场" => {
            Some("原外层锁 owner 的实际进程观察在发布恢复收据期间变化；保留原锁")
        }
        "角色已完成结果不属于原外层控制器或固定运行合同，保留两层锁" => {
            Some("已完成角色结果不属于同一原 controller 或固定运行合同；保留两层锁，不消费其他结果")
        }
        "原API启动环境与冻结交接不同，保留原结果与两层锁" => {
            Some("原 API 启动环境绑定不同于冻结交接；保留原结果与两层锁，不重放")
        }
        "原已成功角色结果与当前完整树不同，保留原结果与两层锁" => {
            Some("原已成功角色结果与当前完整树不一致；保留原结果与两层锁，不补发 start/stop")
        }
        "已成功角色结果归档后完整树变化，保留归档与原外层锁" => {
            Some("角色成功结果归档后完整树变化；保留原归档和外层锁，不能宣称恢复完成")
        }
        "角色 控制锁缺少收据或包含未知文件" => {
            Some("原角色控制锁缺少完整收据或含未知文件；保留原生证据与外层锁，不追加控制")
        }
        "Worker或未知控制器尚有未完成操作，不能推定Worker完整树已停止" => {
            Some("Worker 或未知控制器仍有未完成操作；不能推定 Worker 完整树已停止")
        }
        "API或Worker角色收据、完整树或原缺失事实在观察期间变化" => {
            Some("API/Worker 角色收据、完整树或原缺失事实在观察期间变化；停止继续操作")
        }
        "API控制完整阶段账本在操作期间变化" => {
            Some("API 控制的完整阶段账本发生变化；保留当前控制器和原证据")
        }
        "API控制活动登记或准备合同在操作期间变化" => {
            Some("API 控制的活动登记或准备合同变化；不回退旧成功，也不重签交接")
        }
        "API停止没有取得完整成员关闭证明，保留原操作，不追加停止" => {
            Some("API 停止未取得完整成员关闭证明；保留原操作，不追加停止")
        }
        "API生产者历史在完整树关闭期间变化，拒绝发布停止事件"
        | "API生产者历史在完整代次核验期间变化" => {
            Some("API 生产者历史在完整代次或关闭核验期间变化；不发布新的停止历史")
        }
        "服务进程 PID 无效" | "服务进程创建代次无效" | "服务进程可执行文件必须为绝对路径" => {
            Some("登记进程创建身份字段不合法；核对 PID、创建代次与绝对产物路径，不猜测身份")
        }
        _ => None,
    }
}

pub(crate) fn diagnostic(error: &(dyn Error + 'static)) -> Option<(String, String)> {
    if let Some(error) = error.downcast_ref::<Failure>() {
        return Some((error.phase.details().0.into(), error.to_string()));
    }
    error.source().and_then(diagnostic)
}
