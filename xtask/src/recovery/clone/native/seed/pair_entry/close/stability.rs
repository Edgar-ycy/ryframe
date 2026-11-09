//! 同一 Worker 观察窗口内，连续核对任务排空和终态计数。
use crate::Result;
use serde_json::{Value, json};
use std::{
    thread,
    time::{Duration, Instant},
};

use super::model;

const TIMEOUT: Duration = Duration::from_secs(15 * 60);
const STABLE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_secs(1);

pub(super) enum Outcome {
    Drained(Value),
    TimedOut(Value),
}

pub(super) fn observe(
    mut snapshot: impl FnMut() -> Result<Value>,
    mut check_runtime: impl FnMut() -> Result<()>,
) -> Result<Outcome> {
    let start = Instant::now();
    let mut stable = Stability::default();
    loop {
        check_runtime()?;
        let value = snapshot()?;
        check_runtime()?;
        let elapsed = start.elapsed();
        if let Some(evidence) = stable.sample(&value, elapsed)? {
            return Ok(Outcome::Drained(evidence));
        }
        if elapsed >= TIMEOUT {
            return Ok(Outcome::TimedOut(value));
        }
        thread::sleep(POLL.min(TIMEOUT - elapsed));
    }
}

#[derive(Default)]
struct Stability {
    first: Option<(Value, Duration)>,
    observations: u64,
}

impl Stability {
    fn sample(&mut self, value: &Value, elapsed: Duration) -> Result<Option<Value>> {
        self.observations += 1;
        if !model::drained(value)? {
            self.first = None;
            return Ok(None);
        }
        if let Some((first, started)) = &self.first
            && first["active"] == value["active"]
            && first["terminal_non_success"] == value["terminal_non_success"]
        {
            return Ok((elapsed.saturating_sub(*started) >= STABLE).then(|| {
                json!({"stable_seconds":STABLE.as_secs(),"observations":self.observations,
                    "first_snapshot":first,"last_snapshot":value})
            }));
        }
        self.first = Some((value.clone(), elapsed));
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(active: u64, terminal: u64, time: &str) -> Value {
        let counts = |keys: &[&str], first| {
            let mut fields = keys
                .iter()
                .map(|key| (key.to_string(), json!(0)))
                .collect::<serde_json::Map<_, _>>();
            fields.insert(keys[0].into(), json!(first));
            fields
        };
        json!({"database_time":time,"active":counts(model::ACTIVE, active),
            "terminal_non_success":counts(model::TERMINAL, terminal)})
    }

    #[test]
    fn close_stability_restarts_after_active_tasks_or_terminal_count_changes() {
        let mut tracker = Stability::default();
        let time = "2026-11-09T01:00:00.000000Z";
        for (index, (active, terminal)) in [(0, 1), (1, 1), (0, 1), (0, 2), (0, 2)]
            .into_iter()
            .enumerate()
        {
            assert!(
                tracker
                    .sample(
                        &snapshot(active, terminal, time),
                        Duration::from_secs(index as u64)
                    )
                    .unwrap()
                    .is_none()
            );
        }
        let last = snapshot(0, 2, "2026-11-09T01:00:05.000000Z");
        let evidence = tracker
            .sample(&last, Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(evidence["observations"], 6);
        assert_eq!(evidence["first_snapshot"], snapshot(0, 2, time));
        assert_eq!(evidence["last_snapshot"], last);
    }
}
