//! 只用持有的 Linux 成员句柄证明原身份已经退出。
use crate::{
    Result,
    process::ManagedChild,
    recovery::process_tree::{Member, observe_process_group_member},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    thread,
    time::{Duration, Instant},
};

pub(super) struct Members {
    group: u32,
    live: BTreeMap<u32, Member>,
}

impl Members {
    pub(super) fn new(group: u32, supervisor: &Value) -> Result<Self> {
        let member = observe_process_group_member(group, group)?.ok_or("角色监督进程登记前退出")?;
        if member.identity() != supervisor || !member.verify_group(group)? {
            return Err("角色监督进程登记前退出或创建身份变化".into());
        }
        Ok(Self {
            group,
            live: BTreeMap::from([(group, member)]),
        })
    }

    pub(super) fn sample(&mut self, child: &ManagedChild, observed: &mut Vec<Value>) -> Result<()> {
        // 先核对全部持有身份，不能因进程未出现在组枚举中就视为已退出。
        let mut retired = Vec::new();
        for (&pid, member) in &self.live {
            if !member.verify_group(self.group)? {
                retired.push(pid);
            }
        }
        for pid in retired {
            self.live.remove(&pid);
        }
        for pid in child.member_pids()? {
            if let Some(previous) = self.live.get(&pid) {
                if !previous.exited()? {
                    continue;
                }
                // 枚举时原身份可能刚退出，PID 重用必须重新核对。
                self.live.remove(&pid);
            }
            if let Some(member) = observe_process_group_member(pid, self.group)? {
                if !observed.contains(member.identity()) {
                    observed.push(member.identity().clone());
                }
                self.live.insert(pid, member);
            }
        }
        Ok(())
    }

    pub(super) fn wait_for_exit(self) -> Result<()> {
        let until = Instant::now() + Duration::from_secs(5);
        let mut error = None;
        for member in self.live.into_values() {
            loop {
                match member.exited() {
                    Ok(true) => break,
                    Ok(false) if Instant::now() < until => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Ok(false) => {
                        error.get_or_insert_with(|| "角色完整成员尚未退出".into());
                        break;
                    }
                    Err(failure) => {
                        error.get_or_insert(failure);
                        break;
                    }
                }
            }
        }
        error.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ChildGroup;
    use std::process::{Command, Stdio};

    #[test]
    fn linux_full_stack_member_releases_an_exited_handle_but_keeps_the_record() {
        let mut command = Command::new("/bin/cat");
        command.stdin(Stdio::piped()).stdout(Stdio::null());
        let mut child = ChildGroup::new().unwrap().spawn(command).unwrap();
        let input = child.take_stdin_writer().unwrap().unwrap();
        let pid = child.id();
        let identity = observe_process_group_member(pid, pid)
            .unwrap()
            .unwrap()
            .identity()
            .clone();
        let mut held = Members::new(pid, &identity).unwrap();
        let mut observed = vec![identity.clone()];
        held.sample(&child, &mut observed).unwrap();
        assert!(held.live.contains_key(&pid));

        drop(input);
        let until = Instant::now() + Duration::from_secs(5);
        while !held.live[&pid].exited().unwrap() {
            assert!(Instant::now() < until, "原监督子进程必须退出");
            thread::sleep(Duration::from_millis(10));
        }
        held.sample(&child, &mut observed).unwrap();
        assert!(held.live.is_empty());
        assert_eq!(observed, vec![identity]);
        held.wait_for_exit().unwrap();
        assert!(child.wait().unwrap().success());
    }
}
