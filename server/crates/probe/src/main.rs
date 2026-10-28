use gem_json::{FromJson, ToJson, json};
use gem_proto::{MergeBy, PullRequest, Repository, Timestamp};

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
}
