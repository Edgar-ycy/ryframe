use super::{
    failure::{Failure, NativeResult},
    process::Cancellation,
    request::endpoint,
};
use futures_util::future::join_all;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(crate) fn metric(source: &str, name: &str) -> NativeResult<f64> {
    let values = source
        .lines()
        .filter(|line| {
            line.starts_with(&format!("{name} ")) || line.starts_with(&format!("{name}{{"))
        })
        .map(|line| {
            line.split_whitespace()
                .last()
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .ok_or_else(|| Failure::new("telemetry_metric"))
        })
        .collect::<NativeResult<Vec<_>>>()?;
    if values.is_empty() {
        return Err(Failure::new("telemetry_metric"));
    }
    Ok(values.iter().sum())
}
fn single(source: &str, name: &str) -> NativeResult<f64> {
    let values = source
        .lines()
        .filter(|line| line.starts_with(&format!("{name} ")))
        .collect::<Vec<_>>();
    if values.len() != 1 {
        return Err(Failure::new("database_metric"));
    }
    values[0]
        .split_whitespace()
        .last()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| Failure::new("database_metric"))
}
pub(crate) fn database(source: &str) -> NativeResult<f64> {
    if single(source, "mysql_up")? != 1.0
        || single(
            source,
            "mysql_exporter_collector_success{collector=\"collect.global_status\"}",
        )? != 1.0
    {
        return Err(Failure::new("database_metric_connection"));
    }
    let count = single(source, "mysql_global_status_threads_connected")?;
    if count.fract() != 0.0 || count > 9_007_199_254_740_991.0 {
        return Err(Failure::new("database_metric_count"));
    }
    Ok(count)
}
#[derive(Default)]
pub(crate) struct Collector {
    initial: Option<Vec<[f64; 3]>>,
    previous: Option<Vec<[f64; 3]>>,
    pub peak_memory: f64,
    pub cpu: f64,
    pub peak_connections: f64,
    pub failures: u64,
}
impl Collector {
    pub(crate) fn collect(
        &mut self,
        services: &[String],
        databases: &[String],
    ) -> NativeResult<()> {
        let current = services
            .iter()
            .map(|source| {
                Ok([
                    metric(source, "ryframe_process_cpu_seconds_total")?,
                    metric(source, "ryframe_process_start_time_seconds")?,
                    metric(source, "ryframe_process_resident_memory_bytes")?,
                ])
            })
            .collect::<NativeResult<Vec<_>>>()?;
        if current.is_empty() || databases.is_empty() {
            return Err(Failure::new("telemetry_missing"));
        }
        let connections = databases
            .iter()
            .map(|source| database(source))
            .collect::<NativeResult<Vec<_>>>()?
            .iter()
            .sum::<f64>();
        if self.previous.as_ref().is_some_and(|rows| {
            rows.len() != current.len()
                || rows
                    .iter()
                    .zip(&current)
                    .any(|(old, new)| old[1] != new[1] || new[0] < old[0])
        }) {
            return Err(Failure::new("telemetry_process_restarted"));
        }
        let initial = self.initial.get_or_insert_with(|| current.clone());
        self.cpu = current
            .iter()
            .zip(initial.iter())
            .map(|(new, first)| new[0] - first[0])
            .sum();
        self.peak_memory = self.peak_memory.max(current.iter().map(|row| row[2]).sum());
        self.peak_connections = self.peak_connections.max(connections);
        self.previous = Some(current);
        Ok(())
    }
    pub(crate) fn document(&self) -> Value {
        json!({"peak_resident_memory_bytes":self.peak_memory,"cpu_seconds":self.cpu,"peak_database_connections":self.peak_connections,"collector_failures":self.failures})
    }
}

pub(crate) struct Observer {
    stop: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<NativeResult<Value>>,
}
impl Observer {
    pub(crate) async fn start(config: &Value, cancel: Cancellation) -> NativeResult<Self> {
        crate::http_session::install_crypto_provider()
            .map_err(|_| Failure::new("crypto_provider"))?;
        let client = reqwest::Client::builder()
            .tls_backend_rustls()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| Failure::new("telemetry_client"))?;
        let services = config["bindings"]["metrics_urls"]
            .as_object()
            .ok_or_else(|| Failure::new("telemetry_binding"))?
            .values()
            .map(endpoint)
            .collect::<NativeResult<Vec<_>>>()?;
        let databases = config["bindings"]["database_metrics_urls"]
            .as_array()
            .ok_or_else(|| Failure::new("telemetry_binding"))?
            .iter()
            .map(endpoint)
            .collect::<NativeResult<Vec<_>>>()?;
        let token = match config["bindings"].get("metrics_token") {
            None => None,
            Some(Value::String(value)) if !value.is_empty() && !value.contains(['\r', '\n']) => {
                Some(value.clone())
            }
            _ => return Err(Failure::new("telemetry_authentication")),
        };
        let state = Arc::new(Mutex::new(Collector::default()));
        let reported = Arc::new(Mutex::new(BTreeSet::new()));
        let resources = Resources {
            client,
            services,
            databases,
            token,
            state: state.clone(),
            reported,
        };
        resources.sample(&cancel).await;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let task = tokio::spawn(async move {
            while !stopping.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if !stopping.load(Ordering::Acquire) {
                    resources.sample(&cancel).await;
                }
            }
            resources.sample(&cancel).await;
            Ok(state
                .lock()
                .map_err(|_| Failure::new("telemetry_state"))?
                .document())
        });
        Ok(Self { stop, task })
    }
    pub(crate) async fn stop(self) -> NativeResult<Value> {
        self.stop.store(true, Ordering::Release);
        self.task
            .await
            .map_err(|_| Failure::new("telemetry_task"))?
    }
}
struct Resources {
    client: reqwest::Client,
    services: Vec<url::Url>,
    databases: Vec<url::Url>,
    token: Option<String>,
    state: Arc<Mutex<Collector>>,
    reported: Arc<Mutex<BTreeSet<&'static str>>>,
}
impl Resources {
    async fn sample(&self, cancel: &Cancellation) {
        let samples = join_all(
            self.services
                .iter()
                .map(|url| self.read(url, true, cancel))
                .chain(
                    self.databases
                        .iter()
                        .map(|url| self.read(url, false, cancel)),
                ),
        )
        .await;
        let result = (|| {
            let samples = samples.into_iter().collect::<NativeResult<Vec<_>>>()?;
            let (services, databases) = samples.split_at(self.services.len());
            self.state
                .lock()
                .map_err(|_| Failure::new("telemetry_state"))?
                .collect(services, databases)
        })();
        if let Err(error) = result {
            if let Ok(mut state) = self.state.lock() {
                state.failures += 1;
            }
            if let Ok(mut reported) = self.reported.lock()
                && reported.insert(error.category)
            {
                eprintln!("运行时指标采集失败：{}", error.category);
            }
        }
    }
    async fn read(
        &self,
        url: &url::Url,
        service: bool,
        cancel: &Cancellation,
    ) -> NativeResult<String> {
        cancel
            .during(async {
                let mut request = self.client.get(url.clone());
                if service && let Some(token) = &self.token {
                    request = request.bearer_auth(token);
                }
                let response = request
                    .send()
                    .await
                    .map_err(|_| Failure::new("telemetry_network"))?;
                if !response.status().is_success() {
                    return Err(Failure::new("telemetry_http"));
                }
                response
                    .text()
                    .await
                    .map_err(|_| Failure::new("telemetry_body"))
            })
            .await
    }
}
