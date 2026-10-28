// HTTP without a socket: a request to the router arrives as JSON naming its method, path and body,
// and is answered with the router's body, or with the HTTP status and body when it fails. Nothing
// here knows what carries the requests. `server` serves HTTP/1.1 on a socket too, for MCP.

pub mod server;

use gem_json::{FromJson, ToJson, Value};
use gem_log::{debug, info, warn};

// A handler's answer: the JSON body of a 200, or a status and a body of text.
pub type Reply = Result<Value, (u16, String)>;

// A JSON body for a 200.
pub fn ok<T: ToJson>(body: &T) -> Reply {
    Ok(body.to_json())
}

type Handler = Box<dyn Fn(Value) -> Reply + Send + Sync>;

struct Route {
    method: &'static str,
    path: String,
    handler: Handler,
}

#[derive(Default)]
pub struct Router {
    routes: Vec<Route>,
}

impl Router {
    pub fn new() -> Router {
        Router::default()
    }

    pub fn get<F>(mut self, path: &str, handler: F) -> Router
    where
        F: Fn() -> Reply + Send + Sync + 'static,
    {
        let handler: Handler = Box::new(move |_: Value| handler());
        self.routes.push(Route {
            method: "GET",
            path: path.to_string(),
            handler,
        });
        self
    }

    // A POST whose JSON body is read as `T`, which is rejected as axum's `Json` rejects it: 422
    // when the body is JSON but not a `T`.
    pub fn post<T, F>(mut self, path: &str, handler: F) -> Router
    where
        T: FromJson,
        F: Fn(T) -> Reply + Send + Sync + 'static,
    {
        let handler: Handler = Box::new(move |body: Value| match T::from_json(&body) {
            Ok(request) => handler(request),
            Err(error) => Err((
                422,
                format!("Failed to deserialize the JSON body into the target type: {error}"),
            )),
        });
        self.routes.push(Route {
            method: "POST",
            path: path.to_string(),
            handler,
        });
        self
    }

    // The status and body of the answer to `method` on `path`.
    pub fn handle(&self, method: &str, path: &str, body: Value) -> (u16, String) {
        let mut on_path = self
            .routes
            .iter()
            .filter(|route| route.path == path)
            .peekable();
        if on_path.peek().is_none() {
            return (404, String::new());
        }
        let Some(route) = on_path.find(|route| route.method.eq_ignore_ascii_case(method)) else {
            return (405, String::new());
        };
        match (route.handler)(body) {
            Ok(body) => (200, body.to_string()),
            Err((status, body)) => (status, body),
        }
    }
}

// The router's answer to `request`. A request that is not one fails with 400.
pub fn respond(app: &Router, request: &str) -> Result<String, (u32, String)> {
    let request = parse(request).inspect_err(|(code, message)| {
        warn!("unparseable request", code = code, message = message);
    })?;
    let (status, body) = app.handle(&request.method, &request.path, request.body);
    // `/prs` is asked for after every action and on every reconnect.
    if request.path == "/prs" {
        debug!(
            "request",
            method = request.method,
            path = request.path,
            status = status
        );
    } else {
        info!(
            "request",
            method = request.method,
            path = request.path,
            status = status
        );
    }
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err((status.into(), body))
    }
}

struct HttpRequest {
    method: String,
    path: String,
    body: Value,
}

fn parse(request: &str) -> Result<HttpRequest, (u32, String)> {
    let bad_request = |error: &dyn std::fmt::Display| (400, error.to_string());
    let request = gem_json::parse(request).map_err(|e| bad_request(&e))?;
    let method: String = request.field("method").map_err(|e| bad_request(&e))?;
    let path: String = request.field("path").map_err(|e| bad_request(&e))?;
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(bad_request(&format!("invalid HTTP method {method:?}")));
    }
    if !path.starts_with('/') {
        return Err(bad_request(&format!("invalid URI {path:?}")));
    }
    // The path without its query, as a router matches it.
    let path = path.split('?').next().unwrap_or_default().to_string();
    let body = request.get("body").cloned().unwrap_or_default();
    Ok(HttpRequest { method, path, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gem_json::json;

    struct Echo {
        text: String,
    }

    impl FromJson for Echo {
        fn from_json(value: &Value) -> Result<Echo, gem_json::Error> {
            Ok(Echo {
                text: value.field("text")?,
            })
        }
    }

    fn app() -> Router {
        Router::new()
            .get("/healthz", || Ok(json!({"status": "ok"})))
            .post("/echo", |echo: Echo| match echo.text.as_str() {
                "" => Err((400, "empty".to_string())),
                text => Ok(json!({"text": text})),
            })
    }

    #[test]
    fn routes_by_method_and_path() {
        let request = |r: Value| respond(&app(), &r.to_string());
        assert_eq!(
            request(json!({"method": "GET", "path": "/healthz", "body": null})),
            Ok(r#"{"status":"ok"}"#.to_string())
        );
        assert_eq!(
            request(json!({"method": "POST", "path": "/echo", "body": {"text": "hi"}})),
            Ok(r#"{"text":"hi"}"#.to_string())
        );
        assert_eq!(
            request(json!({"method": "POST", "path": "/echo", "body": {"text": ""}})),
            Err((400, "empty".to_string()))
        );
        assert_eq!(
            request(json!({"method": "POST", "path": "/echo", "body": {}}))
                .unwrap_err()
                .0,
            422
        );
        assert_eq!(
            request(json!({"method": "GET", "path": "/echo", "body": null}))
                .unwrap_err()
                .0,
            405
        );
        assert_eq!(
            request(json!({"method": "GET", "path": "/nope", "body": null}))
                .unwrap_err()
                .0,
            404
        );
        assert_eq!(respond(&app(), "not json").unwrap_err().0, 400);
    }
}
