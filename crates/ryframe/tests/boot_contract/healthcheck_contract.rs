use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

use ryframe::healthcheck::probe_local_readiness;

fn serve_once(status: &str) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("应绑定临时健康端口");
    let port = listener.local_addr().expect("应读取临时地址").port();
    let response = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let task = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("应收到健康检查连接");
        let mut request = [0_u8; 128];
        let received = stream.read(&mut request).expect("应读取健康检查请求");
        assert!(String::from_utf8_lossy(&request[..received]).starts_with("GET /readyz "));
        stream
            .write_all(response.as_bytes())
            .expect("应写入健康检查响应");
    });
    (port, task)
}

#[test]
fn accepts_ready_response_without_external_http_client() {
    let (port, task) = serve_once("200 OK");
    probe_local_readiness(port).expect("200 响应应通过健康检查");
    task.join().expect("健康检查服务应正常退出");
}

#[test]
fn rejects_non_ready_response() {
    let (port, task) = serve_once("503 Service Unavailable");
    let error = probe_local_readiness(port).expect_err("非 200 响应必须失败");
    assert!(error.to_string().contains("非 200 状态"));
    task.join().expect("健康检查服务应正常退出");
}
