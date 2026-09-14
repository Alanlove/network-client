//! DNS-over-HTTPS transport (RFC 8484): POST wire message to `/dns-query`.

use reqwest::Client;

use crate::common::{SvcError, SvcResult};

pub async fn query(client: &Client, url: &str, wire_query: &[u8]) -> SvcResult<Vec<u8>> {
    let resp = client
        .post(url)
        .header("content-type", "application/dns-message")
        .header("accept", "application/dns-message")
        .body(wire_query.to_vec())
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(SvcError::Dns(format!("DoH http {}", resp.status())));
    }
    Ok(resp.bytes().await?.to_vec())
}
