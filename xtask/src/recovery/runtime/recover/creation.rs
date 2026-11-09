//! 仅采用同一首次启动意图，补齐尚未创建的首代生命周期。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::super::{
    bind::ownership::Owner,
    dataset_authority::{
        commands,
        inputs::Inputs,
        reads::{self, Window},
    },
    generation::{self, STATE},
    registration::runtime_observation,
    status::{Bound, optional_file},
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::{clone::native::resources::common, inputs::document},
};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Creation(Option<Intent>);

#[derive(Debug, PartialEq, Eq)]
struct Intent {
    binding: Value,
    request: Value,
    commands: Value,
    directory: (PathBuf, Value),
}

impl Creation {
    /// 只供父层已核准同绑定的首代旧 start owner 时调用。
    pub(super) fn read(backend: &Path, bound: &Bound, inputs: &mut Inputs) -> Result<Self> {
        Cut::new(backend, bound).read(
            inputs,
            || bound.verify_not_started(backend),
            || bound.verify_closed_roles(),
            |request, inputs| {
                bound.check_stop_authority(&request["authority"])?;
                commands::verify_direct(request, inputs)
            },
        )
    }

    /// 父层传入当前 Recover owner；已存在的 intent 不改写。
    pub(super) fn publish(
        self,
        backend: &Path,
        bound: &Bound,
        owner: &Owner,
        inputs: &mut Inputs,
    ) -> Result<bool> {
        if reads::is_active() {
            return Err("恢复首代状态发布前必须结束原只读窗口".into());
        }
        owner.check()?;
        let window = Window::direct()?;
        let current = Self::read(backend, bound, inputs);
        let current = window.finish_runtime_owner(owner, inputs.finish(current))?;
        drop(window);
        if !self.same_cut(&current, owner.control_directory()?) {
            return Err("取得 Recover owner 后首次启动意图或原运行目录变化".into());
        }
        let Some(intent) = current.0 else {
            owner.check()?;
            return Ok(false);
        };
        let result = Cut::new(backend, bound).materialize(
            &intent,
            inputs,
            || owner.check(),
            || bound.verify_closed_roles(),
        );
        inputs.finish(result.map(|()| true))
    }

    fn same_cut(&self, current: &Self, _control: &Path) -> bool {
        if self == current {
            return true;
        }
        #[cfg(target_os = "linux")]
        if let (Some(before), Some(after)) = (&self.0, &current.0) {
            return before.binding == after.binding
                && before.request == after.request
                && before.commands == after.commands
                && same_control_anchor(&before.directory, &after.directory, _control);
        }
        false
    }
}

#[cfg(target_os = "linux")]
fn same_control_anchor(
    before: &(PathBuf, Value),
    after: &(PathBuf, Value),
    control: &Path,
) -> bool {
    if before.0 != after.0
        || before.1["exists"] != false
        || before.1["anchor"]["path"].as_str().map(Path::new) != Some(control)
    {
        return false;
    }
    // 切换控制 owner 会修改 Linux 控制目录的 ctime；新 owner 下仍固定完整新快照。
    let mut expected = before.1.clone();
    expected["anchor"]["created_ns"] = after.1["anchor"]["created_ns"].clone();
    expected == after.1
}

struct Cut<'a> {
    backend: &'a Path,
    root: &'a Path,
    binding: &'a Value,
}

impl<'a> Cut<'a> {
    fn new(backend: &'a Path, bound: &'a Bound) -> Self {
        Self {
            backend,
            root: &bound.runtime,
            binding: &bound.binding,
        }
    }

    fn read(
        &self,
        inputs: &mut Inputs,
        verify_not_started: impl FnOnce() -> Result<()>,
        verify_closed: impl FnOnce() -> Result<()>,
        verify_commands: impl FnOnce(&Value, &mut Inputs) -> Result<Value>,
    ) -> Result<Creation> {
        self.require_no_state()?;
        let path = generation::creation_path(self.root);
        let present = optional_file(&path)?;
        let intent = if present {
            validate_local_test_path(&path, self.backend, LocalTestPathKind::ExistingFile)?;
            let creation = inputs.read(&path)?;
            let binding = document::binding(&creation);
            if self.model("read-creation", &Value::Null, &binding, &Value::Null)? != creation.value
            {
                return Err("原首次启动意图在核验期间变化".into());
            }
            let request = creation.value["request"].clone();
            let commands = verify_commands(&request, inputs)?;
            let directory = runtime_observation(self.backend, self.root)?;
            verify_closed()?;
            if runtime_observation(self.backend, self.root)? != directory {
                return Err("首次启动目录在三端闭口核验期间变化".into());
            }
            creation.assert_unchanged()?;
            Some(Intent {
                binding,
                request,
                commands,
                directory,
            })
        } else {
            verify_not_started()?;
            None
        };
        self.require_no_state()?;
        if optional_file(&path)? != present {
            return Err("原首次启动意图在观察期间出现或消失".into());
        }
        inputs.finish(Ok(Creation(intent)))
    }

    fn materialize(
        &self,
        intent: &Intent,
        inputs: &Inputs,
        check_owner: impl Fn() -> Result<()>,
        verify_closed: impl Fn() -> Result<()>,
    ) -> Result<()> {
        check_owner()?;
        inputs.finish(Ok(()))?;
        if runtime_observation(self.backend, self.root)? != intent.directory {
            return Err("首次启动目录在补齐前偏离原空目录观察".into());
        }
        let state = self.model(
            "initial-state",
            &intent.request,
            &intent.binding,
            &intent.commands,
        )?;
        let empty = self.prepare_runtime(&intent.directory)?;
        let identity = directory_id(self.root)?;
        verify_closed()?;
        if runtime_observation(self.backend, self.root)? != empty {
            return Err("首代生命周期发布前空目录身份变化".into());
        }
        check_owner()?;
        inputs.finish(Ok(()))?;
        let published = self.model(
            "save-state",
            &state,
            &Value::Null,
            &json!([intent.commands]),
        )?;
        let state_identity = self.check_published(&published, &state, identity)?;
        verify_closed()?;
        if self.check_published(&published, &state, identity)? != state_identity {
            return Err("首代生命周期发布文件在三端复核期间被替换，保留现场".into());
        }
        inputs.finish(Ok(()))?;
        check_owner()
    }

    fn prepare_runtime(&self, before: &(PathBuf, Value)) -> Result<(PathBuf, Value)> {
        if before.1["exists"] == true {
            return Ok(before.clone());
        }
        let parent = self.root.parent().ok_or("首次运行目录缺少父目录")?;
        let anchor = &before.1["anchor"];
        let identity = directory_id(parent)?;
        if anchor["device"].as_u64() != Some(identity.0)
            || anchor["inode"].as_u64() != Some(identity.1)
            || runtime_observation(self.backend, self.root)? != *before
        {
            return Err("首次运行目录的原缺席锚点在补齐前变化".into());
        }
        fs::create_dir(self.root)?;
        if directory_id(parent)? != identity {
            return Err("首次运行目录的父目录身份在创建期间变化".into());
        }
        runtime_observation(self.backend, self.root)
    }

    fn check_published(
        &self,
        binding: &Value,
        state: &Value,
        identity: (u64, u64),
    ) -> Result<(u64, u64)> {
        let mut entries = fs::read_dir(self.root)?;
        let only_state = entries
            .next()
            .transpose()?
            .is_some_and(|entry| entry.file_name() == STATE)
            && entries.next().is_none();
        let state_path = self.root.join(STATE);
        let state_identity =
            common::device_inode_for(&state_path, &fs::symlink_metadata(&state_path)?)?;
        let (_, saved) = document::bound_file(self.backend, binding, "首代生命周期状态")?;
        if !only_state
            || directory_id(self.root)? != identity
            || saved.path != state_path
            || saved.value != *state
            || common::device_inode_for(&state_path, &fs::symlink_metadata(&state_path)?)?
                != state_identity
        {
            return Err("首代生命周期发布后目录、状态或绑定变化，保留现场".into());
        }
        Ok(state_identity)
    }

    fn require_no_state(&self) -> Result<()> {
        if optional_file(&self.root.join(STATE))? {
            return Err("首次启动切点已出现生命周期，不能补写".into());
        }
        Ok(())
    }

    fn model(
        &self,
        action: &str,
        value: &Value,
        previous: &Value,
        commands: &Value,
    ) -> Result<Value> {
        generation::request(&json!({
            "action":action,"root":self.root,"binding":self.binding,
            "value":value,"previous":previous,"commands":commands,"number":1,
        }))
    }
}

fn directory_id(path: &Path) -> Result<(u64, u64)> {
    validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
    common::device_inode_for(path, &fs::symlink_metadata(path)?)
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/runtime/recover/creation.rs"]
mod tests;
