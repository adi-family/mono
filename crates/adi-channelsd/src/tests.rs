use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use adi_channels::Target;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::task::{JoinHandle, JoinSet};

use super::*;

struct Scratch {
    root: PathBuf,
    config: Config,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "adi-channelsd-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        let config = Config::with_root(&root);
        Self { root, config }
    }

    fn service(&self, router_url: &str) -> Arc<Service> {
        Arc::new(Service::new(self.config.clone(), router_url.into(), None))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Exercise the actual parser, origin gate, dispatch, and response writer together.
async fn request(
    service: &Arc<Service>,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, Value) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let service = Arc::clone(service);
    let serving = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        serve(stream, service).await.unwrap();
    });
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n",
        body.len(),
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(body.as_bytes()).await.unwrap();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_string(&mut response))
        .await
        .expect("API response timeout")
        .unwrap();
    serving.await.unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap())
}

async fn get(service: &Arc<Service>, path: &str) -> (u16, Value) {
    request(service, "GET", path, &[], "").await
}

async fn post(service: &Arc<Service>, path: &str, body: Value) -> (u16, Value) {
    request(
        service,
        "POST",
        path,
        &[("Content-Type", "application/json")],
        &body.to_string(),
    )
    .await
}

#[test]
fn only_a_hive_assigned_port_can_start_the_service() {
    assert!(listen_addr(None).is_err());
    assert!(listen_addr(Some("0")).is_err());
    assert!(listen_addr(Some("oops")).is_err());
    assert_eq!(
        listen_addr(Some("8234")).unwrap(),
        "127.0.0.1:8234".parse().unwrap(),
    );
}

#[tokio::test]
async fn serves_health_connections_mutations_and_api_errors() {
    let scratch = Scratch::new("http");
    let service = scratch.service("http://unused.invalid");
    for path in ["/health", "/api/health"] {
        let (status, body) = get(&service, path).await;
        assert_eq!(status, 200);
        assert_eq!(body["service"], "adi-channelsd");
        assert_eq!(body["url"], adi_channels::service::url());
    }
    assert_eq!(
        get(&service, "/api/channels").await,
        (200, json!({ "connections": [] })),
    );
    let connection = service
        .connections
        .create(
            "telegram",
            Target::Agent {
                agent: "assistant".into(),
            },
        )
        .unwrap();
    let (status, body) = get(&service, "/api/channels?live=1").await;
    assert_eq!(status, 200);
    assert_eq!(body["connections"][0]["id"], connection.id);
    assert_eq!(
        get(&service, "/api/channels/status").await,
        (200, json!({ "connected": { "telegram": false } })),
    );
    let (status, body) = post(
        &service,
        "/api/channels/pause",
        json!({ "id": connection.id, "paused": true }),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["connections"][0]["paused"], true);
    let (status, body) = get(&service, &format!("/api/channels/{}", connection.id)).await;
    assert_eq!(status, 200);
    assert_eq!(body["paused"], true);
    assert!(
        service
            .connections
            .require(&connection.id)
            .unwrap()
            .manifest
            .paused
    );

    for (path, expected) in [("/api/channels/missing", 404), ("/api/not-here", 404)] {
        let (status, body) = get(&service, path).await;
        assert_eq!(status, expected);
        assert_eq!(body["ok"], false);
    }
    let (status, body) = post(&service, "/api/channels/pause", json!({})).await;
    assert_eq!(status, 400);
    assert_eq!(body["ok"], false);
}

#[tokio::test]
async fn rejects_browser_origin_and_simple_post_bypasses_before_mutation() {
    let scratch = Scratch::new("origin");
    let service = scratch.service("http://unused.invalid");
    let connection = service
        .connections
        .create(
            "telegram",
            Target::Agent {
                agent: "assistant".into(),
            },
        )
        .unwrap();
    let body = json!({ "id": connection.id, "paused": true }).to_string();
    let cases = [
        (
            vec![
                ("Host", "evil.example"),
                ("Origin", "http://evil.example"),
                ("Content-Type", "application/json"),
            ],
            403,
        ),
        (
            vec![
                ("Origin", "http://evil.example"),
                ("Content-Type", "application/json"),
            ],
            403,
        ),
        (
            vec![
                ("Sec-Fetch-Site", "cross-site"),
                ("Content-Type", "application/json"),
            ],
            403,
        ),
        (vec![("Content-Type", "text/plain")], 415),
        (vec![], 415),
    ];
    for (headers, expected) in cases {
        let (status, response) =
            request(&service, "POST", "/api/channels/pause", &headers, &body).await;
        assert_eq!(status, expected, "{headers:?}");
        assert_eq!(response["ok"], false);
        assert!(
            !service
                .connections
                .require(&connection.id)
                .unwrap()
                .manifest
                .paused
        );
    }
}

/// Keep subscriptions mid-handshake so shutdown must cancel the connect path itself.
struct FakeRouter {
    url: String,
    subscribed: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl FakeRouter {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let subscribed = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let subscribers = Arc::clone(&subscribed);
        let closures = Arc::clone(&closed);
        let task = tokio::spawn(async move {
            let registered = Arc::new(AtomicUsize::new(0));
            let mut requests = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (mut stream, _) = accepted.unwrap();
                        let registered = Arc::clone(&registered);
                        let subscribed = Arc::clone(&subscribers);
                        let closed = Arc::clone(&closures);
                        requests.spawn(async move {
                            let req = http::read_request(&mut stream).await.unwrap().unwrap();
                            match req.route_path() {
                                "/register" => {
                                    assert_eq!(req.method, "POST");
                                    let id = registered.fetch_add(1, Ordering::SeqCst) + 1;
                                    let body = json!({ "token": "test-node-token", "connection": format!("connection-{id}"), "link_code": "test-code" });
                                    http::write_json(&mut stream, 200, &body.to_string()).await.unwrap();
                                }
                                "/subscribe" => {
                                    assert_eq!(req.header("authorization"), Some("Bearer test-node-token"));
                                    subscribed.fetch_add(1, Ordering::SeqCst);
                                    let mut discarded = Vec::new();
                                    stream.read_to_end(&mut discarded).await.unwrap();
                                    closed.fetch_add(1, Ordering::SeqCst);
                                }
                                "/disconnect" => {
                                    assert_eq!(req.method, "POST");
                                    assert_eq!(req.header("authorization"), Some("Bearer test-node-token"));
                                    http::write_json(&mut stream, 200, "{}").await.unwrap();
                                }
                                other => panic!("unexpected router request: {other}"),
                            }
                        });
                    }
                    Some(result) = requests.join_next(), if !requests.is_empty() => { result.unwrap(); }
                }
            }
        });
        Self {
            url,
            subscribed,
            closed,
            task,
        }
    }
}

impl Drop for FakeRouter {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn wait_for(counter: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while counter.load(Ordering::SeqCst) != expected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("router lifecycle transition timed out");
}

#[tokio::test]
async fn connects_restores_and_disconnects_one_client_per_provider() {
    let router = FakeRouter::start().await;
    let scratch = Scratch::new("lifecycle");
    let service = scratch.service(&router.url);
    for id in ["connection-1", "connection-2"] {
        let (status, body) = post(
            &service,
            "/api/channels/connect",
            json!({
                "provider": "telegram", "target": { "kind": "agent", "agent": "assistant" },
            }),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["connection"]["id"], id);
    }
    wait_for(&router.subscribed, 1).await;
    assert_eq!(service.live.status().await.len(), 1);
    service.live.stop_all().await;
    wait_for(&router.closed, 1).await;

    // A fresh daemon uses persisted connections and token, independent of adi-app.
    let restarted = scratch.service(&router.url);
    restarted.reconcile().await.unwrap();
    wait_for(&router.subscribed, 2).await;
    assert_eq!(restarted.live.status().await.len(), 1);
    let (status, body) = post(
        &restarted,
        "/api/channels/disconnect",
        json!({ "id": "connection-1" }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(restarted.live.status().await.len(), 1);
    assert_eq!(router.closed.load(Ordering::SeqCst), 1);

    let (status, body) = post(
        &restarted,
        "/api/channels/disconnect",
        json!({ "id": "connection-2" }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["connections"], json!([]));
    assert!(restarted.live.status().await.is_empty());
    wait_for(&router.closed, 2).await;
    assert!(
        adi_channels::token::load(&restarted.secrets, "telegram")
            .unwrap()
            .is_none()
    );
}
