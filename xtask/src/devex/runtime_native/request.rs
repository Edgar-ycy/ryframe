use super::{
    artifacts,
    configuration::{expand, pointer, scalar},
    failure::{Failure, NativeResult},
    imports::Samples,
    pacing::Pacing,
    process::Cancellation,
};
use base64::Engine;
use futures_util::StreamExt;
use reqwest::{
    Client, Method, Response,
    header::{HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use url::Url;

pub(crate) struct Session<'a> {
    config: &'a Value,
    catalog: &'a BTreeMap<String, Value>,
    identity: Value,
    cookies: BTreeMap<String, String>,
    token: Option<String>,
    client: Client,
    pub requests: u64,
    samples: Option<&'a Samples>,
}

impl<'a> Session<'a> {
    pub(crate) fn new(
        config: &'a Value,
        catalog: &'a BTreeMap<String, Value>,
        identity: Value,
        samples: Option<&'a Samples>,
    ) -> NativeResult<Self> {
        crate::http_session::install_crypto_provider()
            .map_err(|_| Failure::new("crypto_provider"))?;
        let api = endpoint(&config["bindings"]["api_url"])?;
        if let Some(address) = identity.get("client_address") {
            let address = address
                .as_str()
                .and_then(|value| value.parse::<std::net::Ipv4Addr>().ok())
                .ok_or_else(|| Failure::new("identity_address"))?;
            if address.octets()[0] != 198
                || !matches!(address.octets()[1], 18 | 19)
                || !matches!(api.host_str(), Some("127.0.0.1" | "[::1]"))
            {
                return Err(Failure::new("identity_address"));
            }
        }
        let client = Client::builder()
            .tls_backend_rustls()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(timeout(config)?)
            .build()
            .map_err(|_| Failure::new("http_client"))?;
        Ok(Self {
            config,
            catalog,
            identity,
            cookies: BTreeMap::new(),
            token: None,
            client,
            requests: 0,
            samples,
        })
    }
    pub(crate) async fn login(
        &mut self,
        pacing: &Pacing,
        cancel: &Cancellation,
    ) -> NativeResult<()> {
        let password = self.identity["password"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Failure::new("missing_credentials"))?
            .to_owned();
        self.request_once(&json!({"operation": "get_auth_csrf"}), pacing, cancel)
            .await?;
        let result = self
            .request_once(
                &json!({"operation": "post_auth_login", "body": {
            "username": self.identity["username"], "password": password }}),
                pacing,
                cancel,
            )
            .await?;
        self.token = Some(
            pointer(&result, "/data/access_token")?
                .as_str()
                .ok_or_else(|| Failure::new("invalid_response"))?
                .into(),
        );
        Ok(())
    }
    pub(crate) async fn request(
        &mut self,
        step: &Value,
        pacing: &Pacing,
        cancel: &Cancellation,
    ) -> NativeResult<Value> {
        let operation = step["operation"]
            .as_str()
            .ok_or_else(|| Failure::new("invalid_operation"))?;
        if step.get("poll").is_some()
            && self
                .catalog
                .get(operation)
                .is_none_or(|entry| entry["method"] != "GET")
        {
            return Err(Failure::new("write_poll_forbidden"));
        }
        if matches!(
            step["operation"].as_str(),
            Some("post_auth_refresh" | "post_auth_logout")
        ) {
            self.request_once(&json!({"operation": "get_auth_csrf"}), pacing, cancel)
                .await?;
        }
        self.request_once(step, pacing, cancel).await
    }
    async fn request_once(
        &mut self,
        step: &Value,
        pacing: &Pacing,
        cancel: &Cancellation,
    ) -> NativeResult<Value> {
        let name = step["operation"]
            .as_str()
            .ok_or_else(|| Failure::new("invalid_operation"))?;
        let operation = self
            .catalog
            .get(name)
            .ok_or_else(|| Failure::new("invalid_operation"))?;
        let method = operation["method"]
            .as_str()
            .ok_or_else(|| Failure::new("invalid_operation"))?;
        if step.get("poll").is_some() && method != "GET" {
            return Err(Failure::new("write_poll_forbidden"));
        }
        let binary = step.get("binary").is_some();
        if binary
            && (step["binary"] != true
                || method != "GET"
                || name != "get_system_users_import_template")
        {
            return Err(Failure::new("binary_operation_forbidden"));
        }
        let mut url = endpoint(&self.config["bindings"]["api_url"])?;
        let route = route(
            operation["path"]
                .as_str()
                .ok_or_else(|| Failure::new("invalid_operation"))?,
            step,
        )?;
        url = url
            .join(&route)
            .map_err(|_| Failure::new("request_route"))?;
        if let Some(query) = step["query"].as_object() {
            for (key, value) in query {
                if !value.is_null() {
                    url.query_pairs_mut().append_pair(key, &scalar(value));
                }
            }
        }
        let mut headers = self.headers(step, name)?;
        let method =
            Method::from_bytes(method.as_bytes()).map_err(|_| Failure::new("invalid_operation"))?;
        let mut request = self.client.request(method, url);
        if let Some(files) = step.get("multipart") {
            request = request.multipart(multipart(files, self.samples)?);
        } else if let Some(body) = step.get("body") {
            headers.insert("content-type", HeaderValue::from_static("application/json"));
            request = request.json(body);
        }
        let request = request
            .headers(headers)
            .build()
            .map_err(|_| Failure::new("request_configuration"))?;
        pacing
            .before(&self.identity, name, operation, cancel)
            .await?;
        self.requests += 1;
        let result = cancel
            .during(async {
                tokio::time::timeout(timeout(self.config)?, async {
                    let response = self.client.execute(request).await.map_err(|error| {
                        Failure::request(
                            if error.is_timeout() {
                                "request_timeout"
                            } else {
                                "request_failed"
                            },
                            name,
                            None,
                        )
                    })?;
                    self.cookies(response.headers())?;
                    if !response.status().is_success() {
                        return Err(Failure::request(
                            "http_status",
                            name,
                            Some(response.status().as_u16()),
                        ));
                    }
                    if response.status() == reqwest::StatusCode::NO_CONTENT {
                        return Ok(Value::Null);
                    }
                    read_response(response, binary)
                        .await
                        .map_err(|_| Failure::request("invalid_response", name, None))
                })
                .await
                .map_err(|_| Failure::request("request_timeout", name, None))?
            })
            .await?;
        if result
            .get("code")
            .is_some_and(|code| code.as_f64() != Some(200.0))
        {
            return Err(Failure::request("business_response", name, None));
        }
        if name == "post_auth_refresh" {
            self.token = Some(
                pointer(&result, "/data/access_token")?
                    .as_str()
                    .ok_or_else(|| Failure::new("invalid_response"))?
                    .into(),
            );
        }
        Ok(result)
    }
    fn headers(&self, step: &Value, operation: &str) -> NativeResult<HeaderMap> {
        let mut headers = HeaderMap::new();
        let key = match step.get("idempotency_key") {
            None => uuid::Uuid::new_v4().to_string(),
            Some(Value::String(value))
                if operation == "post_system_depts"
                    && uuid::Uuid::parse_str(value).is_ok_and(|uuid| {
                        uuid.get_version_num() == 4 && uuid.to_string() == *value
                    }) =>
            {
                value.clone()
            }
            _ => return Err(Failure::new("invalid_idempotency")),
        };
        let origin = endpoint(&self.config["bindings"]["frontend_url"])?
            .origin()
            .ascii_serialization();
        for (name, value) in [
            (
                "x-tenant-id",
                self.identity["tenant_id"]
                    .as_str()
                    .ok_or_else(|| Failure::new("invalid_identity"))?,
            ),
            ("idempotency-key", &key),
            ("origin", &origin),
        ] {
            insert(&mut headers, name, value)?;
        }
        if let Some(address) = self.identity["client_address"].as_str() {
            insert(&mut headers, "x-forwarded-for", address)?;
        }
        if let Some(token) = &self.token {
            insert(&mut headers, "authorization", &format!("Bearer {token}"))?;
        }
        if !self.cookies.is_empty() {
            insert(
                &mut headers,
                "cookie",
                &self
                    .cookies
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            )?;
        }
        if let Some(csrf) = self.cookies.get("ryframe_csrf") {
            insert(&mut headers, "x-csrf-token", csrf)?;
        }
        Ok(headers)
    }
    fn cookies(&mut self, headers: &HeaderMap) -> NativeResult<()> {
        for cookie in headers.get_all("set-cookie") {
            let pair = cookie
                .to_str()
                .map_err(|_| Failure::new("invalid_response"))?
                .split(';')
                .next()
                .unwrap_or("");
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| Failure::new("invalid_response"))?;
            self.cookies.insert(key.into(), value.into());
        }
        Ok(())
    }
    pub(crate) async fn workflow(
        &mut self,
        workflow: &Value,
        variables: &mut Value,
        pacing: &Pacing,
        cancel: &Cancellation,
    ) -> NativeResult<()> {
        let steps = workflow["steps"]
            .as_array()
            .ok_or_else(|| Failure::new("invalid_workflow"))?;
        for definition in steps {
            let step = expand(definition, variables)?;
            if step["operation"] == "post_auth_login" {
                self.login(pacing, cancel).await?;
                continue;
            }
            let started = tokio::time::Instant::now();
            let final_value = loop {
                let result = self.request(&step, pacing, cancel).await?;
                let Some(poll) = step.get("poll") else {
                    break result;
                };
                let state = pointer(
                    &result,
                    poll["pointer"]
                        .as_str()
                        .ok_or_else(|| Failure::new("invalid_poll"))?,
                )?;
                if *state == poll["equals"] {
                    break result;
                }
                let failed = poll["failures"]
                    .as_array()
                    .is_some_and(|rows| rows.contains(state));
                if failed || started.elapsed() >= timeout(self.config)? {
                    return Err(Failure::request(
                        if failed {
                            "poll_failed"
                        } else {
                            "poll_timeout"
                        },
                        step["operation"].as_str().unwrap_or("unknown"),
                        None,
                    ));
                }
                cancel.sleep(Duration::from_millis(250)).await?;
            };
            if let Some(saved) = step["save"].as_object() {
                for (key, at) in saved {
                    variables[key] = pointer(
                        &final_value,
                        at.as_str().ok_or_else(|| Failure::new("invalid_save"))?,
                    )?
                    .clone();
                }
            }
            if let Some(assertions) = step["expect"].as_array() {
                for assertion in assertions {
                    if pointer(
                        &final_value,
                        assertion["pointer"]
                            .as_str()
                            .ok_or_else(|| Failure::new("invalid_assertion"))?,
                    )? != &assertion["equals"]
                    {
                        return Err(Failure::new("response_assertion"));
                    }
                }
            }
        }
        Ok(())
    }
}

fn timeout(config: &Value) -> NativeResult<Duration> {
    config["contract"]["timeout_ms"]
        .as_u64()
        .map(Duration::from_millis)
        .ok_or_else(|| Failure::new("request_timeout_configuration"))
}
fn insert(headers: &mut HeaderMap, name: &'static str, value: &str) -> NativeResult<()> {
    let mut value = HeaderValue::from_str(value).map_err(|_| Failure::new("request_header"))?;
    if matches!(name, "authorization" | "cookie" | "x-csrf-token") {
        value.set_sensitive(true);
    }
    headers.insert(name, value);
    Ok(())
}
pub(crate) fn endpoint(value: &Value) -> NativeResult<Url> {
    Url::parse(value.as_str().ok_or_else(|| Failure::new("endpoint"))?)
        .ok()
        .filter(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        })
        .ok_or_else(|| Failure::new("endpoint"))
}
fn route(template: &str, step: &Value) -> NativeResult<String> {
    let pattern = regex::Regex::new(r"\{([^}]+)\}").map_err(|_| Failure::new("request_route"))?;
    if !template.starts_with('/') || template.starts_with("//") || template.contains(['?', '#']) {
        return Err(Failure::new("request_route"));
    }
    let mut route = String::new();
    let mut end = 0;
    for captures in pattern.captures_iter(template) {
        let found = captures
            .get(0)
            .ok_or_else(|| Failure::new("request_route"))?;
        route.push_str(&template[end..found.start()]);
        let value = scalar(
            step["path"]
                .get(&captures[1])
                .ok_or_else(|| Failure::new("request_path_parameter"))?,
        );
        let encoded: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
        route.push_str(&encoded.replace('+', "%20"));
        end = found.end();
    }
    route.push_str(&template[end..]);
    Ok(route)
}
fn multipart(files: &Value, samples: Option<&Samples>) -> NativeResult<reqwest::multipart::Form> {
    let mut form = reqwest::multipart::Form::new();
    for file in files
        .as_array()
        .ok_or_else(|| Failure::new("multipart_binding"))?
    {
        if file.get("path_env").is_some() {
            return Err(Failure::new("file_configuration_required"));
        }
        if file.get("sample").is_some() == file.get("path").is_some() {
            return Err(Failure::new("multipart_binding"));
        }
        let content = if let Some(sample) = file["sample"].as_str() {
            samples
                .ok_or_else(|| Failure::new("multipart_sample"))?
                .read(sample)?
        } else {
            artifacts::read(
                std::path::Path::new(
                    file["path"]
                        .as_str()
                        .ok_or_else(|| Failure::new("multipart_binding"))?,
                ),
                16 * 1024 * 1024,
            )?
        };
        let part = reqwest::multipart::Part::bytes(content)
            .file_name(
                file["filename"]
                    .as_str()
                    .ok_or_else(|| Failure::new("multipart_binding"))?
                    .to_owned(),
            )
            .mime_str(
                file["type"]
                    .as_str()
                    .ok_or_else(|| Failure::new("multipart_binding"))?,
            )
            .map_err(|_| Failure::new("multipart_binding"))?;
        form = form.part(
            file["field"]
                .as_str()
                .ok_or_else(|| Failure::new("multipart_binding"))?
                .to_owned(),
            part,
        );
    }
    Ok(form)
}
pub(crate) async fn read_response(response: Response, binary: bool) -> NativeResult<Value> {
    let media = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let json = media.contains("json");
    let mut body = Vec::new();
    let mut bytes = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| Failure::new("invalid_response"))?;
        bytes = bytes
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| Failure::new("response_limit"))?;
        if (binary && bytes > 512 * 1024) || (!binary && json && bytes > 16 * 1024 * 1024) {
            return Err(Failure::new("response_limit"));
        }
        if binary || json {
            body.extend_from_slice(&chunk);
        }
    }
    if binary {
        if bytes == 0 || media.to_lowercase().contains("json") {
            return Err(Failure::new("binary_media_type"));
        }
        Ok(
            json!({"bytes": bytes, "media_type": media, "sha256": artifacts::sha(&body), "base64": base64::engine::general_purpose::STANDARD.encode(body)}),
        )
    } else if json {
        serde_json::from_slice(&body).map_err(|_| Failure::new("invalid_response"))
    } else {
        Ok(json!({"bytes": bytes}))
    }
}
