// HTTP/1.1 on a socket, enough for a JSON API on the loopback interface: each request has a
// Content-Length body or none, each connection gets its own thread, and keep-alive is honored.

use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    time::Duration,
};

use gem_log::warn;

// Larger bodies are refused, as axum refuses bodies over 2 MB.
const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_HEADERS: usize = 100;
// An idle keep-alive connection is closed after this.
const IDLE: Duration = Duration::from_secs(60);

pub struct Request {
    pub method: String,
    // With its query, if any.
    pub path: String,
    // Names in lowercase.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn json(status: u16, body: &gem_json::Value) -> Response {
        Response::new(status)
            .header("content-type", "application/json")
            .body(body.to_string().into_bytes())
    }

    pub fn header(mut self, name: &str, value: &str) -> Response {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn body(mut self, body: Vec<u8>) -> Response {
        self.body = body;
        self
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "",
    }
}

// Answers every connection `listener` accepts with `handler`, until accepting fails.
pub fn serve<H>(listener: TcpListener, handler: H) -> std::io::Result<()>
where
    H: Fn(Request) -> Response + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    loop {
        let (stream, _) = listener.accept()?;
        let handler = handler.clone();
        std::thread::spawn(move || {
            if let Err(error) = connection(stream, &*handler) {
                warn!("HTTP connection failed", error = error);
            }
        });
    }
}

fn connection<H>(stream: TcpStream, handler: &H) -> std::io::Result<()>
where
    H: Fn(Request) -> Response,
{
    stream.set_read_timeout(Some(IDLE))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    loop {
        let request = match read_request(&mut reader) {
            Ok(Some(request)) => request,
            Ok(None) => return Ok(()),
            Err(Refused(status)) => {
                write_response(&mut writer, Response::new(status), false)?;
                return Ok(());
            }
        };
        let close = request
            .header("connection")
            .is_some_and(|c| c.eq_ignore_ascii_case("close"));
        let response = handler(request);
        write_response(&mut writer, response, !close)?;
        if close {
            return Ok(());
        }
    }
}

// A request answered with this status without reaching the handler.
struct Refused(u16);

// None when the connection closed before a request began.
fn read_request<R: BufRead>(reader: &mut R) -> Result<Option<Request>, Refused> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) | Err(_) => return Ok(None),
        Ok(_) => {}
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path), Some(_version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(Refused(400));
    };
    let (method, path) = (method.to_string(), path.to_string());
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).map_err(|_| Refused(400))? == 0 {
            return Err(Refused(400));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').ok_or(Refused(400))?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        if headers.len() > MAX_HEADERS {
            return Err(Refused(400));
        }
    }
    let length = match headers.iter().find(|(n, _)| n == "content-length") {
        Some((_, value)) => value.parse::<usize>().map_err(|_| Refused(400))?,
        None if headers.iter().any(|(n, _)| n == "transfer-encoding") => return Err(Refused(400)),
        None => 0,
    };
    if length > MAX_BODY {
        return Err(Refused(413));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|_| Refused(400))?;
    Ok(Some(Request {
        method,
        path,
        headers,
        body,
    }))
}

fn write_response<W: Write>(
    writer: &mut W,
    response: Response,
    keep_alive: bool,
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!("content-length: {}\r\n", response.body.len()));
    if !keep_alive {
        head.push_str("connection: close\r\n");
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(&response.body);
    writer.write_all(&bytes)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, net::Ipv4Addr};

    #[test]
    fn answers_over_a_socket() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            serve(listener, |request: Request| {
                let auth = request
                    .header("Authorization")
                    .unwrap_or_default()
                    .to_string();
                Response::new(200).body(
                    format!(
                        "{} {} {auth} {}",
                        request.method,
                        request.path,
                        request.body.len()
                    )
                    .into_bytes(),
                )
            })
        });
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .write_all(
                b"POST /mcp HTTP/1.1\r\nAuthorization: Bearer t\r\nContent-Length: 2\r\n\r\n{}",
            )
            .unwrap();
        stream
            .write_all(b"GET /x HTTP/1.1\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains("POST /mcp Bearer t 2"), "{response}");
        assert!(response.ends_with("GET /x  0"), "{response}");
        assert_eq!(response.matches("HTTP/1.1 200 OK").count(), 2);
    }
}
