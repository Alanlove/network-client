//! HTTP(S) subscription fetcher. Only does network I/O — decoding and
//! parsing are separate stages so failures can never touch stored nodes.

use std::time::Duration;

use crate::common::{mask_url, SvcError, SvcResult};

#[derive(Clone)]
pub struct Downloader {
    client: reqwest::Client,
}

impl Downloader {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .user_agent("NetworkClient/0.1")
            .build()
            .expect("reqwest client");
        Self { client }
    }

    pub async fn fetch(&self, url: &str) -> SvcResult<String> {
        tracing::debug!(url = %mask_url(url), "downloading subscription");
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(SvcError::Subscription("subscription url must be http(s)".into()));
        }
        let resp = self.client.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(SvcError::Subscription(format!(
                "http status {}",
                resp.status()
            )));
        }
        let body = resp.text().await?;
        if body.trim().is_empty() {
            return Err(SvcError::Subscription("empty subscription body".into()));
        }
        Ok(body)
    }
}

impl Default for Downloader {
    fn default() -> Self {
        Self::new()
    }
}
