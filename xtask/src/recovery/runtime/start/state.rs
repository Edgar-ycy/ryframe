//! 在既有恢复运行控制锁内发布首代和后继生命周期，不启动角色。
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::prepare::Prepared;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{
        clone::native::{digest, json as strict, resources::common},
        fixture_control::services::lifecycle::readonly::check_restore_completion,
        inputs::document::{self, Document},
        runtime::{
            bind::ownership::Owner,
            dataset_authority::{
                inputs::Inputs,
                reads::{self, Window},
            },
            generation::{self, LAUNCH, ROLES, STATE},
            registration::runtime_observation,
            status::{Bound, optional_file, roles},
            stop,
        },
    },
};

type Identity = (u64, u64);

pub(super) struct State {
    runtime: Runtime,
    saved: Saved,
    commands: Value,
    number: u64,
    directory: PathBuf,
    root_identity: Identity,
    directory_identity: Identity,
}

impl State {
    pub(super) fn begin(
        backend: &Path,
        prepared: &Prepared,
        owner: &Owner,
        inputs: &mut Inputs,
    ) -> Result<Self> {
        let runtime = Runtime {
            backend: backend.to_owned(),
            root: prepared.bound.runtime.clone(),
            binding: prepared.bound.binding.clone(),
        };
        let window = Window::direct()?;
        owner.check()?;
        let before = runtime.before(&prepared.bound, inputs);
        let before = window.finish_runtime_owner(owner, inputs.finish(before))?;
        drop(window);

        let result = match before {
            Before::First(empty) => {
                runtime.first(&prepared.request, &prepared.commands, empty, owner, inputs)
            }
            Before::Later(previous) => {
                runtime.append(&prepared.request, &prepared.commands, previous, owner)
            }
        };
        inputs.finish(result)
    }

    pub(super) fn generation(&self) -> &Value {
        &self.saved.document.value["generations"][self.number as usize - 1]
    }

    pub(super) fn number(&self) -> u64 {
        self.number
    }

    pub(super) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(super) fn update(
        &mut self,
        owner: &Owner,
        mutate: impl FnOnce(&mut Value) -> Result<()>,
    ) -> Result<()> {
        self.check(owner)?;
        let mut next = self.saved.document.value.clone();
        let generation = &mut next["generations"][self.number as usize - 1];
        mutate(generation)?;
        unchanged_intent(self.generation(), generation)?;
        let window = Window::direct()?;
        let validated = self.runtime.validate(&next, &self.commands);
        window.finish_runtime_owner(owner, validated)?;
        drop(window);

        self.check(owner)?;
        self.saved = self.runtime.save(
            &next,
            &document::binding(&self.saved.document),
            &self.commands,
        )?;
        self.check(owner)
    }

    pub(super) fn publish_launch(&mut self, owner: &Owner) -> Result<Value> {
        if reads::is_active() {
            return Err("恢复运行启动收据发布前必须结束原只读窗口".into());
        }
        self.check(owner)?;
        let generation = self.generation();
        if generation["status"] != "starting"
            || !generation["launch"].is_null()
            || ROLES
                .iter()
                .any(|role| generation["roles"][*role]["state"] != "running")
        {
            return Err("只有当前三个角色均已登记运行才可发布启动收据".into());
        }
        let binding = self.runtime.model(
            "write-launch",
            &self.directory,
            generation,
            &Value::Null,
            &self.commands[self.number as usize - 1],
            self.number,
        )?;
        let launch = Saved::read(&self.runtime.backend, &self.directory.join(LAUNCH))?;
        if document::binding(&launch.document) != binding {
            return Err("启动收据在写后读取时变化，保留现场".into());
        }
        self.update(owner, |generation| {
            generation["launch"] = binding.clone();
            generation["status"] = json!("running");
            Ok(())
        })?;
        launch.check()?;
        Ok(binding)
    }

    fn check(&self, owner: &Owner) -> Result<()> {
        owner.check()?;
        same_directory(&self.runtime.root, self.root_identity)?;
        same_directory(&self.directory, self.directory_identity)?;
        self.saved.check()?;
        owner.check()
    }
}

fn unchanged_intent(original: &Value, next: &Value) -> Result<()> {
    for key in ["number", "directory", "request"] {
        if next[key] != original[key] {
            return Err("运行状态更新改变已登记启动意图，保留现场".into());
        }
    }
    for role in ROLES {
        for key in ["operation_id", "command", "log"] {
            if next["roles"][role][key] != original["roles"][role][key] {
                return Err("运行状态更新改变已登记角色操作，保留现场".into());
            }
        }
    }
    Ok(())
}

struct Runtime {
    backend: PathBuf,
    root: PathBuf,
    binding: Value,
}

enum Before {
    First((PathBuf, Value)),
    Later(Previous),
}

struct Previous {
    saved: Saved,
    commands: Value,
    root_identity: Identity,
    directory: PathBuf,
    directory_identity: Identity,
}

impl Runtime {
    fn before(&self, bound: &Bound, inputs: &mut Inputs) -> Result<Before> {
        let state_path = self.root.join(STATE);
        if !optional_file(&state_path)? {
            let creation_path = generation::creation_path(&self.root);
            if optional_file(&creation_path)? {
                return Err("原首次启动意图仍存在，须显式恢复原控制器".into());
            }
            let before = runtime_observation(&self.backend, &self.root)?;
            bound.verify_not_started(&self.backend)?;
            if runtime_observation(&self.backend, &self.root)? != before
                || optional_file(&creation_path)?
                || optional_file(&state_path)?
            {
                return Err("首代运行目录、生命周期或创建意图在读取期间变化".into());
            }
            return Ok(Before::First(before));
        }

        let root_identity = entry_identity(&self.root, true)?;
        let saved = Saved::read(&self.backend, &state_path)?;
        let value = &saved.document.value;
        let commands = bound.validate_generation(value, inputs)?;
        let creation = inputs.bound(&value["creation_intent"])?;
        if creation.value["request"] != value["generations"][0]["request"] {
            return Err("首代状态与原创建意图绑定的启动请求不同".into());
        }
        let generation = strict::array(&value["generations"], "恢复原运行代次")?
            .last()
            .ok_or("恢复运行生命周期缺少代次")?;
        if !matches!(
            generation["status"].as_str(),
            Some("stopped" | "start_failed")
        ) {
            return Err("上一恢复运行代次尚未明确停止".into());
        }
        bound.check_stop_authority(&generation["request"]["authority"])?;
        let directory = generation::path(&generation["directory"])?;
        let directory_identity = entry_identity(&directory, true)?;
        verify_previous(&self.backend, &directory, generation, inputs)?;
        bound.verify_closed_roles()?;
        same_directory(&self.root, root_identity)?;
        same_directory(&directory, directory_identity)?;
        saved.check()?;
        Ok(Before::Later(Previous {
            saved,
            commands,
            root_identity,
            directory,
            directory_identity,
        }))
    }

    fn first(
        self,
        request: &Value,
        commands: &Value,
        empty: (PathBuf, Value),
        owner: &Owner,
        inputs: &mut Inputs,
    ) -> Result<State> {
        let parent = self.root.parent().ok_or("首次恢复运行目录缺少父目录")?;
        let parent_identity = entry_identity(parent, true)?;
        self.require_empty(&empty, false)?;
        let intent_path = generation::creation_path(&self.root);
        owner.check()?;
        inputs.finish(Ok(()))?;
        if optional_file(&intent_path)? {
            return Err("原首次启动意图仍存在，须显式恢复原控制器".into());
        }
        let intent = self.model(
            "create-intent",
            &self.root,
            request,
            &Value::Null,
            &Value::Null,
            1,
        )?;
        inputs.bound(&intent)?;
        self.require_empty(&empty, true)?;
        same_directory(parent, parent_identity)?;
        if empty.1["exists"] == false {
            owner.check()?;
            fs::create_dir(&self.root)?;
            same_directory(parent, parent_identity)?;
        }
        let root_identity = entry_identity(&self.root, true)?;
        let empty_root = runtime_observation(&self.backend, &self.root)?;
        if empty.1["exists"] == true && empty_root != empty {
            return Err("首代恢复运行原空目录发生变化，保留现场".into());
        }
        same_directory(&self.root, root_identity)?;
        let value = self.model("initial-state", &self.root, request, &intent, commands, 1)?;
        inputs.finish(Ok(()))?;
        self.require_empty(&empty_root, false)?;
        self.publish_generation(value, None, json!([commands]), root_identity, owner)
    }

    fn append(
        self,
        request: &Value,
        commands: &Value,
        previous: Previous,
        owner: &Owner,
    ) -> Result<State> {
        let Previous {
            saved,
            commands: mut history,
            root_identity,
            directory,
            directory_identity,
        } = previous;
        let mut value = saved.document.value.clone();
        let generations = value["generations"]
            .as_array_mut()
            .ok_or("恢复运行缺少代次")?;
        let number = u64::try_from(generations.len())?
            .checked_add(1)
            .ok_or("恢复代次编号超限")?;
        generations.push(self.model(
            "new-generation",
            &self.root,
            request,
            &Value::Null,
            commands,
            number,
        )?);
        history
            .as_array_mut()
            .ok_or("恢复运行缺少历代命令")?
            .push(commands.clone());
        same_directory(&directory, directory_identity)?;
        let state = self.publish_generation(value, Some(&saved), history, root_identity, owner)?;
        same_directory(&directory, directory_identity)?;
        Ok(state)
    }

    fn require_empty(&self, before: &(PathBuf, Value), wrote_intent: bool) -> Result<()> {
        let actual = runtime_observation(&self.backend, &self.root)?;
        let mut expected = before.clone();
        if wrote_intent && expected.1["exists"] == false {
            // 首次 intent 与缺席运行根同父目录，创建 intent 会改变 Linux 的 ctime。
            expected.1["anchor"]["created_ns"] = actual.1["anchor"]["created_ns"].clone();
        }
        if actual != expected {
            return Err("首代恢复运行空目录或原缺席锚点发生变化，保留现场".into());
        }
        Ok(())
    }

    fn publish_generation(
        self,
        value: Value,
        previous: Option<&Saved>,
        commands: Value,
        root_identity: Identity,
        owner: &Owner,
    ) -> Result<State> {
        let generation = strict::array(&value["generations"], "恢复运行代次")?
            .last()
            .ok_or("恢复运行缺少当代")?;
        let number = generation::integer(&generation["number"])?;
        let directory = generation::path(&generation["directory"])?;
        require_absent(&directory)?;
        owner.check()?;
        same_directory(&self.root, root_identity)?;
        let previous = if let Some(previous) = previous {
            self.validate(&value, &commands)?;
            previous.check()?;
            document::binding(&previous.document)
        } else {
            runtime_observation(&self.backend, &self.root)?;
            Value::Null
        };
        owner.check()?;
        same_directory(&self.root, root_identity)?;
        let saved = self.save(&value, &previous, &commands)?;
        same_directory(&self.root, root_identity)?;
        saved.check()?;
        owner.check()?;
        fs::create_dir(&directory)?;
        let state = State {
            runtime: self,
            saved,
            commands,
            number,
            directory_identity: entry_identity(&directory, true)?,
            directory,
            root_identity,
        };
        state
            .runtime
            .validate(&state.saved.document.value, &state.commands)?;
        state.check(owner)?;
        Ok(state)
    }

    fn validate(&self, value: &Value, commands: &Value) -> Result<()> {
        self.model(
            "validate-state",
            &self.root,
            value,
            &Value::Null,
            commands,
            1,
        )?;
        Ok(())
    }

    fn save(&self, value: &Value, previous: &Value, commands: &Value) -> Result<Saved> {
        let binding = self.model("save-state", &self.root, value, previous, commands, 1)?;
        let saved = Saved::read(&self.backend, &self.root.join(STATE))?;
        if saved.document.value != *value || document::binding(&saved.document) != binding {
            return Err("恢复运行状态在发布后发生变化，保留现场".into());
        }
        Ok(saved)
    }

    fn model(
        &self,
        action: &str,
        root: &Path,
        value: &Value,
        previous: &Value,
        commands: &Value,
        number: u64,
    ) -> Result<Value> {
        generation::request(&json!({
            "action":action,"root":root,"binding":self.binding,
            "value":value,"previous":previous,"commands":commands,"number":number,
        }))
    }
}

fn verify_previous(
    backend: &Path,
    directory: &Path,
    generation: &Value,
    inputs: &mut Inputs,
) -> Result<()> {
    let observed = roles::observe(backend, generation, inputs)?;
    if ROLES
        .iter()
        .any(|role| observed[*role]["state"] == "running")
    {
        return Err("上一恢复运行代次仍有原角色在运行".into());
    }
    stop::verify_pending(backend, directory, generation, &observed)?;
    for role in ROLES {
        let item = &observed[role];
        if item["state"] == "stopped" {
            let completion = inputs.bound(&item["completion"])?;
            check_restore_completion(&item["tree"], &completion.value)?;
            for member in strict::array(&completion.value["members"], "原已停止成员")? {
                stop::require_exited(member)?;
            }
        }
    }
    Ok(())
}

struct Saved {
    document: Document,
    identity: Identity,
}

impl Saved {
    fn read(backend: &Path, path: &Path) -> Result<Self> {
        let identity = entry_identity(path, false)?;
        let document = document::read(path, backend, "原恢复运行状态或启动收据")?;
        let saved = Self { document, identity };
        saved.check()?;
        Ok(saved)
    }

    fn check(&self) -> Result<()> {
        if entry_identity(&self.document.path, false)? != self.identity {
            return Err("原恢复运行状态或启动收据的文件身份变化，保留现场".into());
        }
        self.document.assert_unchanged()?;
        if entry_identity(&self.document.path, false)? != self.identity {
            return Err("原恢复运行文件在复核期间被替换，保留现场".into());
        }
        Ok(())
    }
}

fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("新恢复运行代次目录已存在，保留现场".into()),
    }
}

fn same_directory(path: &Path, expected: Identity) -> Result<()> {
    if entry_identity(path, true)? != expected {
        return Err("恢复运行根或代次目录身份变化，保留现场".into());
    }
    Ok(())
}

fn entry_identity(path: &Path, directory: bool) -> Result<Identity> {
    let kind = if directory {
        LocalTestPathKind::ExistingDirectory
    } else {
        LocalTestPathKind::ExistingFile
    };
    validate_absolute_path(path, kind)?;
    let metadata = fs::symlink_metadata(path)?;
    if digest::linked(&metadata)
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err("恢复运行状态文件或目录类型发生变化，保留现场".into());
    }
    common::device_inode_for(path, &metadata)
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/runtime/start/state.rs"]
mod tests;
