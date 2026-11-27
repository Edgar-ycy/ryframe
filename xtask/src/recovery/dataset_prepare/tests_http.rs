use super::super::{
    existing, prepare,
    session::{Client, Pacer},
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

type CapturedRequests = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

struct Server {
    url: String,
    requests: CapturedRequests,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(handler: impl Fn(&str, &[u8]) -> (u16, Vec<u8>) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let shutdown = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        let thread = thread::spawn(move || {
            while !shutdown.load(Ordering::SeqCst) {
                let (mut socket, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 8192];
                let end = loop {
                    let n = socket.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(i) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while bytes.len() < end + length {
                    let n = socket.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let body = bytes[end..end + length].to_vec();
                observed
                    .lock()
                    .unwrap()
                    .push((headers.clone(), body.clone()));
                let (status, body) = handler(&headers, &body);
                // 状态零表示服务端已处理请求但主动断开响应。
                if status == 0 {
                    continue;
                }
                write!(socket,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nSet-Cookie: ryframe_csrf=csrf; Path=/\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                socket.write_all(&body).unwrap();
            }
        });
        Self {
            url,
            requests,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let result = self.thread.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}
fn catalog() -> std::collections::BTreeMap<String, Value> {
    [
        ("get_auth_csrf", "GET", "/csrf"),
        ("post_auth_login", "POST", "/login"),
        ("get_auth_context", "GET", "/context"),
        ("put_auth_profile_password", "PUT", "/password"),
        ("post_auth_refresh", "POST", "/refresh"),
        ("post_auth_logout", "POST", "/logout"),
        ("post_system_posts", "POST", "/posts"),
        ("get_system_posts", "GET", "/posts"),
        ("get_system_posts_by_id", "GET", "/posts/{id}"),
        ("post_common_upload", "POST", "/upload"),
        ("get_common_file_download", "GET", "/download"),
        ("get_platform_capabilities", "GET", "/capabilities"),
        ("post_platform_product_plans", "POST", "/plans"),
        (
            "post_platform_product_plans_by_plan_id_versions",
            "POST",
            "/plans/{plan_id}/versions",
        ),
        (
            "post_platform_product_plans_by_plan_id_versions_by_version_publish",
            "POST",
            "/plans/{plan_id}/versions/{version}/publish",
        ),
    ]
    .into_iter()
    .map(|(id, method, path)| (id.into(), json!({"method":method,"path":path})))
    .collect()
}

#[test]
fn existing_verification_reads_all_tenants_collects_subjects_and_clears_bearer_for_logout() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};

    const CONTENT: &[u8] = b"source verification sample";
    let server = Server::new(move |headers, _| {
        let tenant = headers
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("x-tenant-id"))
            })
            .map(|(_, value)| value.trim())
            .unwrap_or("system");
        let has_bearer = headers.lines().any(|line| {
            line.to_ascii_lowercase()
                .starts_with("authorization: bearer ")
        });
        if headers.starts_with("POST /logout ") {
            assert!(!has_bearer);
        } else if headers.starts_with("GET /posts/1 ") || headers.starts_with("GET /context ") {
            assert!(has_bearer);
        } else if headers.starts_with("GET /download?") {
            assert!(has_bearer);
            return (200, CONTENT.to_vec());
        }
        let data = if headers.starts_with("GET /context ") {
            full_context(headers)
        } else if headers.starts_with("POST /login ") {
            let claims = json!({"sub":"9007199254740993","tenant_id":tenant,"username":if tenant == "system" {"admin"} else {"owner"},"token_type":"access","user_authorization_version":7});
            let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
            json!({"access_token":format!("header.{payload}.signature")})
        } else if headers.starts_with("GET /posts/1 ") {
            json!({"code":"sample","name":"恢复样本"})
        } else {
            json!({})
        };
        (
            200,
            serde_json::to_vec(&json!({"code":200,"data":data})).unwrap(),
        )
    });
    let target =
        json!({"scope_id":"current-source","api_url":server.url,"frontend_url":server.url});
    let sample_file = json!({"file_path":"source/sample.txt","bytes":CONTENT.len(),
                             "sha256":hex::encode(Sha256::digest(CONTENT))});
    let identities = (0..11).map(|index| {
        json!({"tenant_id":if index == 0 {"system".into()} else {format!("origin-{index:02}")},"username":if index == 0 {"admin"} else {"owner"},"posts":[{"id":"1","code":"sample","name":"恢复样本"}],"files":[sample_file]})
    }).collect::<Vec<_>>();
    let counts = runtime()
        .block_on(existing::verify_at(
            &target,
            &identities,
            &catalog(),
            "memory-only-password",
            existing::Options {
                network: 18,
                request_interval_ms: 0,
                collect_subjects: true,
            },
            || Ok(()),
        ))
        .unwrap();
    assert_eq!((counts.tenants, counts.posts, counts.files), (11, 11, 11));
    assert_eq!(counts.subjects.len(), 11);
    assert!(
        counts
            .subjects
            .iter()
            .all(|subject| subject["user_authorization_version"] == 7)
    );
    let calls = server.requests.lock().unwrap();
    assert!(
        !calls
            .iter()
            .any(|(headers, _)| headers.starts_with("PUT /password "))
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(headers, _)| headers.starts_with("GET /context "))
            .count(),
        11
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(headers, _)| headers.starts_with("POST /login "))
            .count(),
        11
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(headers, _)| headers.starts_with("POST /logout "))
            .count(),
        11
    );
}
fn create_client(server: &Server) -> Client {
    let mut plan = super::plan();
    plan["source"]["api_url"] = json!(server.url);
    let mut client =
        Client::new(&plan, "source", 0, &catalog(), "fixture-memory-password").unwrap();
    client.pacer = Pacer::new(0);
    client
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn dataset_http_records_each_post_refreshes_and_checks_exact_count() {
    let server = Server::new(|headers, body| {
        let response = if headers.starts_with("GET /context ") {
            json!({"code":200,"data":full_context(headers)})
        } else if headers.starts_with("POST /posts ") {
            let value: Value = serde_json::from_slice(body).unwrap();
            assert!(value["sort"].as_u64().unwrap() <= 999);
            json!({"code":200,"data":{"id":(value["code"].as_str().unwrap().rsplit('-').next().unwrap().parse::<u64>().unwrap()+1).to_string(),"code":value["code"],"name":value["name"]}})
        } else if headers.starts_with("GET /posts?") {
            json!({"code":200,"data":{"total":1001}})
        } else {
            json!({"code":200,"data":{"access_token":"fixture-token"}})
        };
        (200, serde_json::to_vec(&response).unwrap())
    });
    let temp = super::directory();
    let mut client = create_client(&server);
    runtime().block_on(async {
        client.login().await.unwrap();
        let samples = prepare::seed_posts(
            &mut client,
            "restore-case",
            "system",
            1001,
            temp.path(),
            &AtomicBool::new(false),
        )
        .await
        .unwrap();
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[2]["code"], "restore-case-1000");
    });
    let records = std::fs::read_to_string(temp.path().join("created.ndjson")).unwrap();
    assert_eq!(records.lines().count(), 1001);
    let calls = server.requests.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|(h, _)| h.starts_with("POST /refresh "))
            .count(),
        10
    );
    assert!(
        calls
            .last()
            .unwrap()
            .0
            .starts_with("GET /posts?code=restore-case&page=1&page_size=1 ")
    );
    assert!(calls.iter().all(|(headers, _)| {
        headers
            .to_ascii_lowercase()
            .contains("x-forwarded-for: 198.18.20.1")
    }));
}
#[test]
fn dataset_http_upload_and_download_preserve_exact_bytes_and_no_retry() {
    let content = prepare::sample_content("fixture", 1, 1024);
    let expected = content.clone();
    let server = Server::new(move |headers, body| {
        if headers.starts_with("GET /context ") {
            return (
                200,
                serde_json::to_vec(&json!({"code":200,"data":full_context(headers)})).unwrap(),
            );
        }
        if headers.starts_with("POST /upload ") {
            assert!(body.windows(expected.len()).any(|v| v == expected));
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer fixture-token")
            );
            return (
                200,
                serde_json::to_vec(
                    &json!({"code":200,"data":[{"file_id":"1","file_path":"source/file"}]}),
                )
                .unwrap(),
            );
        }
        if headers.starts_with("GET /download?") {
            return (200, expected.clone());
        }
        (
            200,
            serde_json::to_vec(&json!({"code":200,"data":{"access_token":"fixture-token"}}))
                .unwrap(),
        )
    });
    let mut client = create_client(&server);
    runtime().block_on(async {
        client.login().await.unwrap();
        client
            .upload("fixture.txt".into(), content.clone())
            .await
            .unwrap();
        let file =
            json!({"file_path":"source/file","bytes":1024,"sha256":super::model::sha(&content)});
        client.download(&file).await.unwrap();
        let mut invalid = file;
        invalid["sha256"] = json!("0".repeat(64));
        assert!(client.download(&invalid).await.is_err());
    });
    let rejected = Server::new(|_, _| (429, b"{}".to_vec()));
    let mut client = create_client(&rejected);
    runtime().block_on(async {
        assert!(client.login().await.is_err());
    });
    assert_eq!(rejected.requests.lock().unwrap().len(), 1);
}
#[test]
fn pacing_covers_every_request_and_invalid_post_result_never_publishes_record() {
    let server = Server::new(|headers, _| {
        let value = if headers.starts_with("GET /context ") {
            json!({"data":full_context(headers)})
        } else if headers.starts_with("POST /posts ") {
            json!({"data":{"id":"1","code":"wrong"}})
        } else {
            json!({"data":{"access_token":"token"}})
        };
        (200, serde_json::to_vec(&value).unwrap())
    });
    let mut client = create_client(&server);
    let temp = super::directory();
    runtime().block_on(async {
        client.login().await.unwrap();
        assert!(
            prepare::seed_posts(
                &mut client,
                "case",
                "system",
                1,
                temp.path(),
                &AtomicBool::new(false)
            )
            .await
            .is_err()
        );
        let mut pacer = Pacer::new(15);
        let began = tokio::time::Instant::now();
        pacer.wait().await;
        pacer.wait().await;
        assert!(began.elapsed() >= Duration::from_millis(15));
    });
    assert!(!temp.path().join("created.ndjson").exists());
}

#[test]
fn empty_capability_version_accepts_optional_catalog_and_rejects_invalid_before_writes() {
    for capabilities in [
        json!([]),
        json!([{"key":"system.notice"}]),
        Value::Null,
        json!({}),
        json!("[]"),
    ] {
        let valid = capabilities.is_array();
        let server = Server::new(move |headers, _| {
            let data = if headers.starts_with("GET /context ") {
                full_context(headers)
            } else if headers.starts_with("GET /capabilities ") {
                capabilities.clone()
            } else if headers.starts_with("POST /plans ") {
                json!({"id":"12"})
            } else if headers.starts_with("POST /plans/12/versions ") {
                json!({"id":"13","version":1})
            } else {
                json!({"access_token":"token"})
            };
            (
                200,
                serde_json::to_vec(&json!({"code":200,"data":data})).unwrap(),
            )
        });
        let mut client = create_client(&server);
        let result = runtime().block_on(async {
            client.login().await.unwrap();
            prepare::plan_version(&mut client, &super::plan()).await
        });
        assert_eq!(result.is_ok(), valid);
        let calls = server.requests.lock().unwrap();
        if valid {
            assert_eq!(calls.len(), 7);
            let body: Value = serde_json::from_slice(&calls[5].1).unwrap();
            assert_eq!(body["capabilities"], json!([]));
            assert!(calls[6].0.starts_with("POST /plans/12/versions/1/publish "));
        } else {
            assert_eq!(calls.len(), 4);
            assert!(
                !calls
                    .iter()
                    .any(|(headers, _)| headers.starts_with("POST /plans"))
            );
        }
    }
}

#[test]
fn exact_post_count_mismatch_keeps_creation_evidence_without_success() {
    let server = Server::new(|headers, _| {
        let data = if headers.starts_with("GET /context ") {
            full_context(headers)
        } else if headers.starts_with("POST /posts ") {
            json!({"id":"1","code":"case-0","name":"恢复样本0"})
        } else if headers.starts_with("GET /posts?") {
            json!({"total":99})
        } else {
            json!({"access_token":"token"})
        };
        (
            200,
            serde_json::to_vec(&json!({"code":200,"data":data})).unwrap(),
        )
    });
    let temp = super::directory();
    let mut client = create_client(&server);
    runtime().block_on(async {
        client.login().await.unwrap();
        let error = prepare::seed_posts(
            &mut client,
            "case",
            "system",
            1,
            temp.path(),
            &AtomicBool::new(false),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("数量"));
    });
    assert_eq!(
        std::fs::read_to_string(temp.path().join("created.ndjson"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(!temp.path().join("result.json").exists());
}

fn full_context(headers: &str) -> Value {
    let tenant = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("x-tenant-id"))
        })
        .map(|(_, value)| value.trim())
        .unwrap_or("system");
    json!({"user":{"id":"9007199254740993","tenant_id":tenant,"username":if tenant == "system" {"admin"} else {"owner"},
        "tenant_name":"参考租户","dept_name":null,"nickname":"参考管理员","email":"","phone":"","avatar":null,"preferred_locale":null},
        "password_change_required":false,"is_super_admin":true,"permissions":["*"],"roles":["admin"],
        "authorization_epoch":"1","runtime_epoch":"1","capabilities":[],"business_data":{"state":"active","placement_generation":"1"},"menus":[]})
}

#[path = "../../../tests/unit/recovery/dataset_prepare/activation.rs"]
mod activation;

#[test]
fn context_fixture_uses_the_published_session_context_and_user_schemas() {
    let document: Value = serde_json::from_slice(
        &std::fs::read(crate::workspace::root_dir().join("openapi/openapi.json")).unwrap(),
    )
    .unwrap();
    let schemas = &document["components"]["schemas"];
    let context = &schemas["SessionContextVo"];
    assert_eq!(
        context["properties"]["password_change_required"]["type"],
        "boolean"
    );
    assert!(
        context["required"]
            .as_array()
            .unwrap()
            .contains(&json!("password_change_required"))
    );
    let reference = context["properties"]["user"]["$ref"].as_str().unwrap();
    let user = &schemas[reference.rsplit('/').next().unwrap()];
    let fixture = full_context("");
    let fields = fixture["user"].as_object().unwrap();
    let properties = user["properties"].as_object().unwrap();
    for required in user["required"].as_array().unwrap() {
        assert!(fields.contains_key(required.as_str().unwrap()));
    }
    assert!(fields.keys().all(|name| properties.contains_key(name)));
    assert!(
        !user["properties"]
            .as_object()
            .unwrap()
            .contains_key("password_change_required")
    );
}
