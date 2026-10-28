// The messages the server and frontend exchange, generated from protocol/gem.proto by build.rs,
// with the canonical proto3 JSON mapping the frontend's protobuf-es decodes. The types have the
// shape prost gives them (a message field is an Option, an enum field an i32 with a getter), and
// their JSON is pbjson's: camelCase names, default values left out, 64-bit integers as strings,
// enums by name, and Timestamps as RFC 3339.

mod timestamp;

pub use timestamp::Timestamp;

include!(concat!(env!("OUT_DIR"), "/gem.v1.rs"));

// An integer field, from a JSON number or a string holding one.
fn read_integer<T: TryFrom<i128>>(value: &gem_json::Value) -> Result<T, gem_json::Error> {
    let n: Option<i128> = match value {
        gem_json::Value::String(s) => s.parse().ok(),
        other => (other.as_i64().map(i128::from)).or_else(|| other.as_u64().map(i128::from)),
    };
    n.and_then(|n| T::try_from(n).ok())
        .ok_or_else(|| gem_json::Error::new(format!("expected an integer, found {value}")))
}

// An enum field, from a value's name or its number.
fn read_enum<E: Copy + Into<i32>>(
    value: &gem_json::Value,
    from_name: fn(&str) -> Option<E>,
) -> Result<i32, gem_json::Error> {
    match value {
        gem_json::Value::String(s) => (from_name(s).map(Into::into))
            .ok_or_else(|| gem_json::Error::new(format!("unknown enum value {s:?}"))),
        other => read_integer(other),
    }
}

#[allow(dead_code)]
fn read_each<T>(
    value: &gem_json::Value,
    read: impl Fn(&gem_json::Value) -> Result<T, gem_json::Error>,
) -> Result<Vec<T>, gem_json::Error> {
    let items = (value.as_array())
        .ok_or_else(|| gem_json::Error::new(format!("expected an array, found {}", value.kind())))?;
    items.iter().map(read).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gem_json::{FromJson, ToJson, json};

    #[test]
    fn writes_proto3_json() {
        let pr = PullRequest {
            repository: Some(Repository { owner: "u9g".into(), name: "gem".into() }),
            number: 7,
            merge_by: MergeBy::Admin as i32,
            checks_passing: true,
            created_at: Some(Timestamp { seconds: 1_700_000_000, nanos: 5_000_000 }),
            ..Default::default()
        };
        assert_eq!(
            pr.to_json(),
            json!({
                "repository": {"owner": "u9g", "name": "gem"},
                "number": 7,
                "checksPassing": true,
                "mergeBy": "MERGE_BY_ADMIN",
                "createdAt": "2023-11-14T22:13:20.005Z",
            })
        );
        let check = FailedCheck { workflow_run_id: 12, ..Default::default() };
        assert_eq!(check.to_json(), json!({"workflowRunId": "12"}));
        assert_eq!(MessageAgentResponse {}.to_json(), json!({}));
    }

    #[test]
    fn reads_proto3_json() {
        let pr = PullRequest::from_json(&json!({
            "number": 7,
            "merge_by": "MERGE_BY_APPROVAL",
            "checks": {"total": "3"},
            "createdAt": "2023-11-14T22:13:20Z",
            "unknown": true,
            "title": null,
        }))
        .unwrap();
        assert_eq!(pr.number, 7);
        assert_eq!(pr.merge_by(), MergeBy::Approval);
        assert_eq!(pr.checks.unwrap().total, 3);
        assert_eq!(pr.created_at, Some(Timestamp { seconds: 1_700_000_000, nanos: 0 }));
        assert_eq!(pr.title, "");
        assert_eq!(Agent::from_json(&json!({"state": 2})).unwrap().state(), AgentState::Waiting);
        assert!(PullRequest::from_json(&json!({"number": "x"})).is_err());
    }
}
