//! 原暖态来源的 Windows 持有会话，始末仍完整复核。
use super::{HeldBuilds, Session};
use crate::{
    Result,
    recovery::{
        clone::native::{export::transport::Transport, resources::common},
        source::evidence::resources::warm_redis,
    },
};

impl<T: Transport> Session<'_, T> {
    pub(super) fn enable_held(&mut self) -> Result<()> {
        let request = &self.facts["request"];
        if self.recover || !warm_redis::applies(request) {
            return Ok(());
        }
        let execution = self.execution()?.to_path_buf();
        let expected = common::read_bound(self.backend, &request["backend_build"], "来源产品构建")?;
        // 保存已取得的句柄。若后续源码保护失败，finish 仍复核构建并持有至释放控制锁。
        self.held_builds = Some(HeldBuilds::open(self.backend, request, &expected)?);
        self.scope.hold_windows_sources(&[&execution])?;
        self.checkpoint_full()
    }

    pub(super) fn finish_held(&self, builds: &HeldBuilds) -> Result<()> {
        let builds = builds.finish(self.backend, &self.facts["request"]);
        let original = self.checkpoint_full();
        combine(builds, original)
    }

    fn checkpoint_full(&self) -> Result<()> {
        let original = self.checkpoint_original();
        let source = self.scope.assert_unchanged_full();
        let owner = self.owner.verify().and_then(|()| self.guard.check());
        combine(combine(original, source), owner)
    }
}

fn combine(first: Result<()>, last: Result<()>) -> Result<()> {
    match (first, last) {
        (Ok(()), other) | (other, Ok(())) => other,
        (Err(first), Err(last)) => {
            Err(format!("{first}。来源持有会话的另一项完整复核也失败：{last}").into())
        }
    }
}
