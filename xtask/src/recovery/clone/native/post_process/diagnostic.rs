//! 三模式的窄阶段诊断；任意底层正文仅留在cause链，不进入正式公共输出。
use crate::{Result, recovery::clone::native::lifecycle::stage::StageRequest};
use std::{error::Error, fmt, path::PathBuf};

#[derive(Clone, Copy, Debug)]
pub(super) enum Phase {
    Binding,
    Registration,
    Preparation,
    Amendment,
    Cleanup,
}

struct Failure {
    phase: Phase,
    path: Option<PathBuf>,
    cause: Box<dyn Error>,
    secondary: Option<Box<dyn Error>>,
}

impl fmt::Debug for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("复制后固定请求阶段未确认完成")
    }
}
impl Error for Failure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.cause)
    }
}

pub(super) fn context<T>(request: &StageRequest<'_>, phase: Phase, result: Result<T>) -> Result<T> {
    result.map_err(|cause| {
        let mut failure = match cause.downcast::<Failure>() {
            Ok(failure) => *failure,
            Err(cause) => Failure {
                phase,
                path: None,
                cause,
                secondary: None,
            },
        };
        failure.path = request
            .attempt
            .map(|number| request.run_dir.join(format!("failure-{number:04}.json")));
        Box::new(failure) as Box<dyn Error>
    })
}

pub(super) fn combined(cause: Box<dyn Error>, secondary: Box<dyn Error>) -> Box<dyn Error> {
    Box::new(Failure {
        phase: Phase::Cleanup,
        path: None,
        cause,
        secondary: Some(secondary),
    })
}

pub(crate) fn diagnostic(error: &(dyn Error + 'static)) -> Option<(&'static str, String)> {
    let Some(failure) = error.downcast_ref::<Failure>() else {
        return error.source().and_then(diagnostic);
    };
    let (kind, phase) = match failure.phase {
        Phase::Binding => ("PostCopyBindingRejected", "binding"),
        Phase::Registration => ("PostCopyRegistrationRejected", "registration"),
        Phase::Preparation => ("PostCopyPreparationRejected", "preparation"),
        Phase::Amendment => ("PostCopyAmendmentRejected", "amendment"),
        Phase::Cleanup => ("PostCopyCleanupRejected", "cleanup"),
    };
    let mut message = format!("固定请求阶段={phase}；{}", reason(&*failure.cause));
    if let Some(cause) = &failure.secondary {
        message.push_str(&format!("；收尾同时未确认：{}", reason(&**cause)));
    }
    if let Some(path) = &failure.path {
        message.push_str(&format!("；阶段证据={}", path.display()));
    }
    message.push_str("；未形成验收合格状态；保留原登记、失败与私有配置，核对原阶段和完整前后像，不覆盖、不自动重放；只有原API/Worker完整关闭且原控制结果明确后才按正式amend修订环境或选择新隔离目标");
    Some((kind, message))
}

fn reason(error: &(dyn Error + 'static)) -> String {
    if let Some(value) = error.downcast_ref::<std::io::Error>() {
        return format!(
            "本地文件或系统操作失败，OS码={:?}；核对受管目录、权限及原文件",
            value.raw_os_error()
        );
    }
    if let Some((_, safe)) = super::super::target_verify::diagnostic(error) {
        return safe;
    }
    if let Some(cim) =
        error.downcast_ref::<super::super::seed::source_registration::quiet::CimUnavailable>()
    {
        return cim.to_string();
    }
    if let Some((_, safe)) = super::super::cache::handoff::diagnostic(error) {
        return safe;
    }
    match error.to_string().as_str() {
        "原停止结果不是同一完整API关闭证明" => {
            "原停止结果的受管运行/历史位置、scope或完整关闭状态不符；原收据保持不变".into()
        }
        "环境修订仅接受最新已明确失败的API启动" => {
            "最新API启动不是原prepare之后的明确失败，不能修订".into()
        }
        "旧prepare不是活动登记的API-only交接" => {
            "旧prepare与当前登记或API-only交接绑定不同".into()
        }
        "原失败诊断不属于最新原生启动控制器" => {
            "原失败阶段、控制器或失败证据绑定不符；不能以其他失败授权修订".into()
        }
        "最新原生启动不是原失败Stage控制器和原冻结环境" => {
            "原生角色归档不是原失败控制器或原冻结环境；保留未知现场".into()
        }
        "原stop控制器与角色关闭归档不唯一或缺失" | "原stop控制器没有唯一成功的原生完整关闭结果" => {
            "原stop控制器缺少唯一成功且完整绑定的角色关闭归档".into()
        }
        "完整原生历史未发布同一API代次的关闭事件，保留未知现场" => {
            "原生完整生产者历史没有同一API代次的关闭事件；不得补造事件或自动重放".into()
        }
        "原始登记已有唯一成功结果；继续prepare，不重复发布原始登记" => {
            "原始登记已成功；继续prepare，不再建立第二个原始登记".into()
        }
        "已有固定收据内容不同，拒绝覆盖或自动重放" => {
            "已有固定收据与本次请求不同，禁止覆盖".into()
        }
        "准备API前目标完整复制后像变化" => {
            "目标完整数据库后像不同于原复制，保留现场核对".into()
        }
        "原来源完整库存不再等于复制来源" => {
            "原来源完整库存已漂移，不能采纳旧复制".into()
        }
        "准备API前完整对象范围不同于复制后像" | "准备API前对象GET字节或摘要不同于原复制对象" => {
            "对象完整范围或GET摘要不同于原复制，保留现场核对".into()
        }
        "业务动作或seed已经开始，不能修订API环境绑定" => {
            "业务动作或seed已开始，不具备环境修订资格".into()
        }
        "失败API必须由最新精确stop收尾，未知或后续操作不能采纳" => {
            "缺少最新成功的精确stop关闭证明；保留未知证据，不重放start或stop".into()
        }
        "旧API或Worker完整监督树尚未停止，保留原失败" => {
            "原API或Worker完整监督树尚未关闭，不能修订登记".into()
        }
        "环境修订只能明确重新绑定api_environment，不能更改业务或其他请求字段" => {
            "修订改变了环境绑定以外的固定字段，或没有明确重绑环境文件".into()
        }
        _ => error.source().map(reason).unwrap_or_else(|| {
            "固定请求、来源、配置、原生历史或发布核验未通过；底层原因未列入安全公开类别".into()
        }),
    }
}
