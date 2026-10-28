use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    process::{Command, Stdio},
    sync::{Arc, Condvar, Mutex, RwLock, mpsc},
    thread,
    time::Duration,
};

use gem_json::{FromJson, ToJson, Value, json};
use gem_log::info;
use gem_proto::{MergeBy, PullRequest, Repository, Timestamp};

type Handler = Box<dyn Fn(&Value) -> Result<Value, (u16, String)> + Send + Sync>;

fn main() {
    let text = std::env::args().nth(1).unwrap_or_else(|| r#"{"number":7,"mergeBy":"MERGE_BY_ADMIN","createdAt":"2023-11-14T22:13:20.005Z"}"#.to_string());
    let pr = PullRequest::from_json(&gem_json::parse(&text).unwrap()).unwrap();
    assert_eq!(pr.merge_by(), MergeBy::Admin);
    let mut pr = pr;
    pr.repository = Some(Repository { owner: "u9g".into(), name: "gem".into() });
    let value = json!({"pr": pr, "now": (Timestamp::now().seconds > 0), "list": [1, null, {"a": "b"}]});
    println!("{}", value);
    println!("{}", gem_json::to_string(&pr.to_json()));
    for value in gem_json::parse_stream("{\"a\":1} {\"a\":2}").unwrap() {
        println!("{}", value["a"]);
    }
    info!("probe", n = 3, what = "log");

    // Routes as boxed closures.
    let mut routes: HashMap<String, Handler> = HashMap::new();
    let count = Arc::new(Mutex::new(0u32));
    let seen = count.clone();
    routes.insert(
        "/count".to_string(),
        Box::new(move |body: &Value| {
            *seen.lock().unwrap() += 1;
            Ok(json!({"echo": (body.clone())}))
        }),
    );
    println!("{}", routes["/count"](&json!({"x": 1})).unwrap());

    // A child process over pipes, read line by line.
    let mut child = Command::new("cat").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || BufReader::new(stdout).lines().map(|l| l.unwrap()).collect::<Vec<_>>());
    writeln!(stdin, "{}", json!({"type": "hello"})).unwrap();
    drop(stdin);
    println!("child said {:?}", reader.join().unwrap());
    child.wait().unwrap();
    let output = Command::new("sh").args(["-c", "echo out; echo err >&2; exit 3"]).output().unwrap();
    println!("{} {:?} {:?}", output.status.success(), output.status.code(), String::from_utf8_lossy(&output.stdout));

    // A condition variable with a timeout, and a channel.
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let notifier = pair.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        *notifier.0.lock().unwrap() = true;
        notifier.1.notify_all();
    });
    let (lock, cvar) = &*pair;
    let (flag, timeout) = cvar.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |f| !*f).unwrap();
    println!("notified {} timed out {}", *flag, timeout.timed_out());
    let (tx, rx) = mpsc::channel();
    thread::scope(|s| {
        for i in 0..3 {
            let tx = tx.clone();
            s.spawn(move || tx.send(i).unwrap());
        }
    });
    drop(tx);
    let mut got: Vec<i32> = rx.iter().collect();
    got.sort();
    println!("{got:?}");
    let cache: RwLock<Option<String>> = RwLock::new(None);
    *cache.write().unwrap() = Some("x".into());
    println!("{:?}", cache.read().unwrap().clone());

    // HTTP over TCP.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let body = "{\"ok\":true}";
        write!(stream, "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    println!("{}", response.lines().last().unwrap());
    println!("{}", std::env::current_exe().unwrap().display());
}
