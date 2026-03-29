use anyhow::Result;
use serde::Deserialize;
use std::time::Duration;

/// Response shape expected from the HTTP routing API.
#[derive(Debug, Deserialize)]
struct HttpRoutingResponse {
    trunk: Option<String>,
}

/// Perform an HTTP GET to `url` with `destination` and `caller` as query
/// parameters.
///
/// Expects a JSON response with an optional `trunk` field. Returns
/// `Ok(Some(trunk_name))` when the response contains a trunk, `Ok(None)` when
/// the trunk field is absent or null, and `Err` on network/timeout/parse
/// errors.
///
/// The request times out after 5 seconds.
pub async fn http_query_lookup(
    client: &reqwest::Client,
    url: &str,
    destination: &str,
    caller: &str,
) -> Result<Option<String>> {
    let response = client
        .get(url)
        .query(&[("destination", destination), ("caller", caller)])
        .timeout(Duration::from_secs(5))
        .send()
        .await?
        .error_for_status()?
        .json::<HttpRoutingResponse>()
        .await?;

    Ok(response.trunk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_http_query_returns_trunk_from_json() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/route"))
            .and(query_param("destination", "+14155551234"))
            .and(query_param("caller", "+10000000001"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"trunk": "trunk-http"})),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/route", server.uri());
        let result = http_query_lookup(&client, &url, "+14155551234", "+10000000001").await;
        assert_eq!(result.unwrap(), Some("trunk-http".to_string()));
    }

    #[tokio::test]
    async fn test_http_query_no_trunk_in_response_returns_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/route"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"other": "value"})),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/route", server.uri());
        let result = http_query_lookup(&client, &url, "+14155551234", "+10000000001").await;
        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test]
    async fn test_http_query_server_error_returns_err() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/route"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/route", server.uri());
        let result = http_query_lookup(&client, &url, "+14155551234", "+10000000001").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_http_query_null_trunk_returns_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/route"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"trunk": null})),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/route", server.uri());
        let result = http_query_lookup(&client, &url, "+14155551234", "+10000000001").await;
        assert_eq!(result.unwrap(), None);
    }
}
