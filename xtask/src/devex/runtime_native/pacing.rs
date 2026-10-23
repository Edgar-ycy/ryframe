use super::{
    Input, artifacts, configuration,
    failure::{Failure, NativeResult},
    process::Cancellation,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::Mutex as AsyncMutex;

type Counts = BTreeMap<String, [f64; 3]>;
type ClientLocks = BTreeMap<String, Arc<AsyncMutex<()>>>;
struct State {
    starts: BTreeMap<String, Instant>,
    principals: BTreeMap<String, String>,
    clients: ClientLocks,
    measurement: Counts,
}
pub(crate) struct Pacing {
    pacing: Value,
    authority: Value,
    state: Mutex<State>,
    directory: PathBuf,
    closed: bool,
}

impl Pacing {
    pub(crate) fn new(config: &Value, input: &Input, directory: &Path) -> NativeResult<Self> {
        let started = Instant::now();
        let mut authority = None;
        let prepared = (|| {
            let binding = &config["bindings"]["pacing"];
            if binding.as_object().is_none_or(|rows| {
                rows.len() != 2
                    || !rows.contains_key("python")
                    || !rows.contains_key("login_budget_state")
            }) || ["python", "login_budget_state"].iter().any(|key| {
                binding[*key]
                    .as_str()
                    .is_none_or(|path| !Path::new(path).is_absolute())
            }) {
                return Err(Failure::new("pacing_binding"));
            }
            let config_dir = config["bindings"]["app_config_dir"].as_str().map(Path::new);
            let loaded = crate::rate_limit::authority(&input.backend, config_dir, &input.app_env)
                .map_err(|_| Failure::new("pacing_authority"))?;
            let loaded =
                serde_json::to_value(loaded).map_err(|_| Failure::new("pacing_authority"))?;
            authority = Some(loaded.clone());
            let catalog = configuration::catalog(&input.backend)?;
            super::super::runtime_pacing::validate_authority(
                &config["contract"]["pacing"],
                &loaded,
                &catalog,
            )
            .map_err(|_| Failure::new("pacing_authority"))?;
            if configuration::hash(&loaded)?
                != config["contract"]["pacing"]["authority_sha256"]
                    .as_str()
                    .unwrap_or("")
            {
                return Err(Failure::new("pacing_authority_digest"));
            }
            Ok(loaded)
        })();
        artifacts::write(
            &directory.join("pacing-authority.json"),
            &json!({"format_version":1, "success":prepared.is_ok(),
            "failure":prepared.as_ref().err().map(|error| error.category), "duration_ms":super::milliseconds(started.elapsed()),
            "actual_sha256":authority.as_ref().map(configuration::hash).transpose()?, "expected_sha256":config["contract"]["pacing"]["authority_sha256"], "authority":authority}),
        )?;
        Ok(Self {
            pacing: config["contract"]["pacing"].clone(),
            authority: prepared?,
            directory: directory.into(),
            closed: false,
            state: Mutex::new(State {
                starts: BTreeMap::new(),
                principals: BTreeMap::new(),
                clients: BTreeMap::new(),
                measurement: BTreeMap::new(),
            }),
        })
    }
    #[cfg(test)]
    pub(crate) fn fixture(pacing: Value, authority: Value, directory: &Path) -> Self {
        Self {
            pacing,
            authority,
            directory: directory.into(),
            closed: false,
            state: Mutex::new(State {
                starts: BTreeMap::new(),
                principals: BTreeMap::new(),
                clients: BTreeMap::new(),
                measurement: BTreeMap::new(),
            }),
        }
    }
    pub(crate) async fn prepare_sample(&self, cancel: &Cancellation) -> NativeResult<()> {
        let started = Instant::now();
        let started_at = chrono::Utc::now().to_rfc3339();
        let requested = self.pacing["sample_prepare_wait_ms"]
            .as_u64()
            .ok_or_else(|| Failure::new("pacing_contract"))?;
        let result = cancel
            .sleep(std::time::Duration::from_millis(requested))
            .await;
        artifacts::write(
            &self.directory.join("sample-preparation.json"),
            &json!({"format_version":1,
            "reason":"fixed_rate_limit_window", "started_at":started_at, "finished_at":chrono::Utc::now().to_rfc3339(), "requested_ms":requested,
            "duration_ms":super::milliseconds(started.elapsed()), "authority_sha256":self.pacing["authority_sha256"],
            "success":result.is_ok(), "failure":result.as_ref().err().map(|error| error.category)}),
        )?;
        result
    }
    pub(crate) async fn before(
        &self,
        identity: &Value,
        name: &str,
        operation: &Value,
        cancel: &Cancellation,
    ) -> NativeResult<()> {
        let address = identity["client_address"]
            .as_str()
            .ok_or_else(|| Failure::new("pacing_identity"))?;
        let principal = json!([
            identity["tenant_id"],
            identity["username"]
                .as_str()
                .ok_or_else(|| Failure::new("pacing_identity"))?
                .trim()
                .to_lowercase()
        ])
        .to_string();
        let lock = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| Failure::new("pacing_state"))?;
            if state
                .principals
                .get(&principal)
                .is_some_and(|previous| previous != address)
            {
                return Err(Failure::new("pacing_identity"));
            }
            state.principals.insert(principal.clone(), address.into());
            state.clients.entry(address.into()).or_default().clone()
        };
        let _guard = cancel.during(async { Ok(lock.lock().await) }).await?;
        let keys = self.keys(address, &principal, name, operation)?;
        let before = Instant::now();
        let wait = {
            let state = self
                .state
                .lock()
                .map_err(|_| Failure::new("pacing_state"))?;
            keys.iter()
                .filter_map(|(key, gap)| {
                    state.starts.get(key).map(|start| {
                        (*start + std::time::Duration::from_millis(*gap))
                            .saturating_duration_since(before)
                    })
                })
                .max()
                .unwrap_or_default()
        };
        cancel.sleep(wait).await?;
        let sent = Instant::now();
        if sent.duration_since(before) < wait {
            return Err(Failure::new("pacing_clock"));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| Failure::new("pacing_state"))?;
        for (key, _) in keys {
            state.starts.insert(key, sent);
        }
        let row = state.measurement.entry(name.into()).or_default();
        row[0] += 1.0;
        row[1] += super::milliseconds(wait);
        row[2] += super::milliseconds(sent.duration_since(before));
        Ok(())
    }
    fn keys(
        &self,
        address: &str,
        principal: &str,
        name: &str,
        operation: &Value,
    ) -> NativeResult<Vec<(String, u64)>> {
        let request = self.pacing["request_interval_ms"]
            .as_u64()
            .ok_or_else(|| Failure::new("pacing_contract"))?;
        let declared = self.pacing["operation_intervals_ms"].get(name);
        let interval = request.max(declared.and_then(Value::as_u64).unwrap_or(0));
        let mut keys = vec![(format!("ip:{address}"), request)];
        if declared.is_some() {
            keys.push((format!("operation:{address}:{name}"), interval));
        }
        if self.authority["user"]["enabled"] == true {
            keys.push((format!("user:{principal}"), request));
        }
        if let Some(rule) =
            super::super::runtime_pacing::matched_rule(operation, &self.authority["api"]["rules"])
        {
            keys.push((format!("rule:{address}:{rule}"), interval));
        }
        if name == "post_auth_login" {
            keys.push((format!("login:{principal}"), interval));
            keys.push((format!("login-ip:{address}"), interval));
        }
        Ok(keys)
    }
    pub(crate) fn close(&mut self) -> NativeResult<()> {
        if self.closed {
            return Err(Failure::new("pacing_closed"));
        }
        self.closed = true;
        let state = self
            .state
            .lock()
            .map_err(|_| Failure::new("pacing_state"))?;
        let measurement: BTreeMap<_, _> = state
            .measurement
            .iter()
            .map(|(name, row)| {
                (
                    name,
                    json!({"requests":row[0],"requested_ms":row[1],"duration_ms":row[2]}),
                )
            })
            .collect();
        artifacts::write(
            &self.directory.join("pacing-summary.json"),
            &json!({"format_version":1,"pacing":self.pacing,
            "measurement":measurement,"preparation":{},"login_budget":{"waits":0,"requested_ms":0}}),
        )
    }
}
