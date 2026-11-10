//! Search result score contract checks against the published OpenAPI schema.

use serde_json::{Value as JsonValue, json};

use super::common::{OPENAPI, assert_component_invalid, assert_component_valid};

#[test]
fn normalized_language_contract_requires_an_explicit_request() {
    let document: JsonValue = serde_yaml::from_str(OPENAPI).unwrap();
    let language = &document["components"]["schemas"]["NormalizedQuery"]["properties"]["language"];
    assert_eq!(language["type"], json!(["string", "null"]));
    assert!(
        language["description"]
            .as_str()
            .unwrap()
            .contains("explicitly requested")
    );
    assert!(document["paths"]["/api/v1/search"]["post"]["responses"]["200"]["content"]
        ["application/json"]["examples"]["english"]["value"]["normalized_query"]["language"]
        .is_null());
}

#[test]
fn station_result_accepts_metadata_scores_above_one() {
    let document: JsonValue = serde_yaml::from_str(OPENAPI).unwrap();
    let score = &document["components"]["schemas"]["StationResult"]["properties"]["score"];
    assert_eq!(score["minimum"], json!(0));
    assert!(score.get("maximum").is_none());

    let result = json!({
        "id": "test-station",
        "name": "Test Station",
        "stream_url": "https://example.com/radio.mp3",
        "homepage_url": null,
        "favicon_url": null,
        "tags": [],
        "language": null,
        "country_code": null,
        "codec": null,
        "bitrate_kbps": null,
        "score": 1.5,
        "reason": "Matched catalog metadata.",
        "health": "healthy"
    });
    assert_component_valid(&document, "StationResult", &result);
    let mut negative = result;
    negative["score"] = json!(-0.1);
    assert_component_invalid(&document, "StationResult", &negative, "negative score");
}
