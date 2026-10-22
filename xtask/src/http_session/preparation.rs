use std::{collections::BTreeMap, time::Duration};

use reqwest::{
    Client, Method, Request,
    header::{HeaderMap, HeaderValue},
};
use serde_json::Value;
use url::{Host, Url};
use uuid::Uuid;

use super::{RequestMetadata, SessionError, error::valid_operation};

#[path = "route.rs"]
mod route;

pub(super) struct Configuration {
    api: Url,
    origin: HeaderValue,
    tenant: HeaderValue,
    forwarded: Option<HeaderValue>,
    catalog: BTreeMap<String, Value>,
    pub(super) username: String,
    pub(super) password: Option<String>,
    pub(super) timeout: Duration,
}

pub(super) struct Plan {
    pub(super) metadata: RequestMetadata,
    url: Url,
    idempotency: String,
    body: Option<Value>,
}

impl Configuration {
    pub(super) fn new(
        config: &Value,
        catalog: BTreeMap<String, Value>,
        identity: &Value,
    ) -> Result<Self, SessionError> {
        let api = endpoint(config.pointer("/bindings/api_url"))?;
        let frontend = endpoint(config.pointer("/bindings/frontend_url"))?;
        let origin = header(
            &frontend.origin().ascii_serialization(),
            "invalid_configuration",
        )?;
        let timeout = config
            .pointer("/contract/timeout_ms")
            .and_then(Value::as_u64)
            .filter(|value| (1..=u32::MAX as u64).contains(value))
            .ok_or_else(|| invalid("invalid_configuration"))?;
        for (operation, entry) in &catalog {
            if !valid_operation(operation)
                || !route::valid(entry["path"].as_str().unwrap_or(""))
                || !matches!(
                    entry["method"].as_str(),
                    Some("GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS")
                )
            {
                return Err(invalid("invalid_operation"));
            }
        }
        let username = text(&identity["username"], "invalid_identity")?.to_owned();
        let tenant = header(
            text(&identity["tenant_id"], "invalid_identity")?,
            "invalid_identity",
        )?;
        let password = identity["password"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let forwarded = forwarded(identity.get("client_address"), &api)?;
        Ok(Self {
            api,
            origin,
            tenant,
            forwarded,
            catalog,
            username,
            password,
            timeout: Duration::from_millis(timeout),
        })
    }

    pub(super) fn prepare(&self, step: &Value) -> Result<Plan, SessionError> {
        let operation = step
            .get("operation")
            .and_then(Value::as_str)
            .filter(|value| valid_operation(value))
            .ok_or_else(|| invalid("invalid_operation"))?;
        let entry = self
            .catalog
            .get(operation)
            .ok_or_else(|| invalid("invalid_operation"))?;
        let error = |category| SessionError::new(category, Some(operation), None);
        let fields = step.as_object().ok_or_else(|| error("invalid_request"))?;
        if fields.keys().any(|field| {
            !matches!(
                field.as_str(),
                "operation" | "path" | "query" | "body" | "idempotency_key"
            )
        }) {
            return Err(error("unsupported_step"));
        }
        let method = entry["method"]
            .as_str()
            .ok_or_else(|| error("invalid_operation"))?;
        if !matches!(method, "GET" | "POST" | "PUT") {
            return Err(error("unsupported_step"));
        }
        if method == "GET" && step.get("body").is_some() {
            return Err(error("invalid_request"));
        }
        let template = entry["path"]
            .as_str()
            .ok_or_else(|| error("invalid_operation"))?;
        let path =
            route::expand(template, step.get("path")).map_err(|_| error("invalid_request"))?;
        let mut url = self.api.join(&path).map_err(|_| error("invalid_request"))?;
        if url.origin() != self.api.origin() {
            return Err(error("invalid_request"));
        }
        route::query(&mut url, step.get("query")).map_err(|_| error("invalid_request"))?;
        let idempotency = match step.get("idempotency_key") {
            None => Uuid::new_v4().to_string(),
            Some(Value::String(value))
                if operation == "post_system_depts" && canonical_uuid(value) =>
            {
                value.clone()
            }
            Some(_) => return Err(error("invalid_request")),
        };
        Ok(Plan {
            metadata: RequestMetadata {
                operation: operation.into(),
                method: method.into(),
                route: template.into(),
            },
            url,
            idempotency,
            body: step.get("body").cloned(),
        })
    }

    pub(super) fn request(
        &self,
        client: &Client,
        plan: &Plan,
        cookies: &BTreeMap<String, String>,
        token: Option<&str>,
    ) -> Result<Request, SessionError> {
        let operation = plan.metadata.operation.as_str();
        let error = |_| SessionError::new("invalid_request", Some(operation), None);
        let mut headers = HeaderMap::new();
        headers.insert("x-tenant-id", self.tenant.clone());
        headers.insert("origin", self.origin.clone());
        headers.insert(
            "idempotency-key",
            HeaderValue::from_str(&plan.idempotency).map_err(error)?,
        );
        if let Some(forwarded) = &self.forwarded {
            headers.insert("x-forwarded-for", forwarded.clone());
        }
        if let Some(token) = token {
            headers.insert(
                "authorization",
                secret_header(&format!("Bearer {token}")).map_err(error)?,
            );
        }
        if !cookies.is_empty() {
            let cookie = cookies
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            headers.insert("cookie", secret_header(&cookie).map_err(error)?);
        }
        if let Some(csrf) = cookies.get("ryframe_csrf") {
            headers.insert("x-csrf-token", secret_header(csrf).map_err(error)?);
        }
        let method = Method::from_bytes(plan.metadata.method.as_bytes())
            .map_err(|_| SessionError::new("invalid_operation", Some(operation), None))?;
        let mut request = client.request(method, plan.url.clone()).headers(headers);
        if let Some(body) = &plan.body {
            request = request.json(body);
        }
        request
            .build()
            .map_err(|_| SessionError::new("invalid_request", Some(operation), None))
    }
}

fn endpoint(value: Option<&Value>) -> Result<Url, SessionError> {
    let url = value
        .and_then(Value::as_str)
        .and_then(|value| Url::parse(value).ok())
        .filter(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        })
        .ok_or_else(|| invalid("invalid_configuration"))?;
    Ok(url)
}

fn forwarded(value: Option<&Value>, api: &Url) -> Result<Option<HeaderValue>, SessionError> {
    let Some(value) = value else { return Ok(None) };
    let address = value
        .as_str()
        .and_then(|value| value.parse::<std::net::Ipv4Addr>().ok())
        .filter(|address| {
            let bytes = address.octets();
            bytes[0] == 198 && matches!(bytes[1], 18 | 19)
        })
        .ok_or_else(|| invalid("invalid_identity"))?;
    let loopback = match api.host() {
        Some(Host::Ipv4(address)) => address == std::net::Ipv4Addr::LOCALHOST,
        Some(Host::Ipv6(address)) => address == std::net::Ipv6Addr::LOCALHOST,
        _ => false,
    };
    if !loopback {
        return Err(invalid("invalid_identity"));
    }
    Ok(Some(header(&address.to_string(), "invalid_identity")?))
}

fn text<'a>(value: &'a Value, category: &'static str) -> Result<&'a str, SessionError> {
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(category))
}

fn header(value: &str, category: &'static str) -> Result<HeaderValue, SessionError> {
    HeaderValue::from_str(value).map_err(|_| invalid(category))
}

fn secret_header(value: &str) -> Result<HeaderValue, reqwest::header::InvalidHeaderValue> {
    let mut value = HeaderValue::from_str(value)?;
    value.set_sensitive(true);
    Ok(value)
}

fn canonical_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|uuid| {
        uuid.get_version_num() == 4
            && uuid.to_string() == value
            && matches!(value.as_bytes().get(19), Some(b'8' | b'9' | b'a' | b'b'))
    })
}

fn invalid(category: &'static str) -> SessionError {
    SessionError::new(category, None, None)
}
