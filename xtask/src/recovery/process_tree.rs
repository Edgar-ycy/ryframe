//! 平台原生整树所有权。monitor 持有成员内核句柄直到收口。
#[cfg(target_os = "linux")]
#[path = "process_tree/linux.rs"]
mod platform;
#[cfg(not(any(windows, target_os = "linux")))]
#[path = "process_tree/unsupported.rs"]
mod platform;
#[cfg(windows)]
#[path = "process_tree/windows.rs"]
mod platform;

pub(in crate::recovery) use platform::{Group, supported};
#[cfg(target_os = "linux")]
pub(in crate::recovery) use platform::{Member, exited, observe_process_group_member, signal};

#[derive(Debug)]
struct Rejected(&'static str);

impl std::fmt::Display for Rejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for Rejected {}

pub(in crate::recovery) fn rejection_reason(
    error: &(dyn std::error::Error + 'static),
) -> Option<&'static str> {
    error.downcast_ref::<Rejected>().map(|error| error.0)
}

#[cfg(any(windows, target_os = "linux"))]
fn check(valid: bool, reason: &'static str) -> crate::Result<()> {
    if valid {
        Ok(())
    } else {
        Err(Box::new(Rejected(reason)))
    }
}
