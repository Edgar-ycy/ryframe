use ryframe_config::AppConfig;
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    Layer,
    filter::{LevelFilter, Targets},
    layer::{Context, SubscriberExt},
};

/// 只安装该真实场景所需的监督器事件；全局订阅使后台 spawn 也可见。
pub(super) fn install(config: &AppConfig) -> Result<(), String> {
    let mut secrets = Vec::new();
    for connection in std::iter::once(&config.database.primary).chain(
        config
            .database
            .replicas
            .iter()
            .map(|replica| &replica.connection),
    ) {
        let url = connection.connection_url();
        if let Some((_, rest)) = url.split_once("://")
            && let Some((credentials, _)) = rest.split_once('@')
        {
            secrets.extend(credentials.split(':').map(str::to_owned));
        }
        secrets.extend([
            url,
            connection.username.clone(),
            connection.password.clone(),
        ]);
    }
    let escaped = secrets
        .iter()
        .map(|value| {
            let quoted = format!("{value:?}");
            quoted[1..quoted.len() - 1].to_owned()
        })
        .collect::<Vec<_>>();
    secrets.extend(escaped);
    secrets.retain(|value| !value.is_empty());
    secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
    secrets.dedup();
    let events = Events { secrets }
        .with_filter(Targets::new().with_target("ryframe::boot::datasource", LevelFilter::DEBUG));
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(events))
        .map_err(|_| "无法安装副本监督器专用诊断订阅；拒绝无诊断继续真实场景".into())
}

struct Events {
    secrets: Vec<String>,
}

impl<S: Subscriber> Layer<S> for Events {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        for secret in &self.secrets {
            fields.0 = fields.0.replace(secret, "[REDACTED]");
        }
        eprintln!(
            "replica supervisor {}: {}",
            event.metadata().level(),
            fields.0
        );
    }
}

struct Fields(String);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if matches!(
            field.name(),
            "message" | "replica" | "error" | "timeout_secs"
        ) {
            use std::fmt::Write;
            let _ = write!(self.0, "{}={value:?} ", field.name());
        }
    }
}
