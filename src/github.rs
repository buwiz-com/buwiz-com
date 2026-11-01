//! Buwiz Bot's star count on GitHub, for the GitHub links on buwiz.com.
//! Read from the public API without a token (60 requests an hour per
//! address; this asks at most every [`REFRESH`]), and kept like
//! [`crate::downloads::Downloads`] keeps `latest.json`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::Deserialize;

/// The repository whose stars are shown.
pub const REPO: &str = "buwiz-bot/buwiz-bot";

/// How long a fetched count is used before asking again.
const REFRESH: Duration = Duration::from_secs(10 * 60);

/// Pages wait for it; they should not wait long.
const TIMEOUT: Duration = Duration::from_secs(3);

/// GitHub refuses API requests without a `User-Agent`.
const USER_AGENT: &str = "buwiz.com (+https://buwiz.com)";

/// [`REPO`]'s `stargazers_count`, fetched when asked for and kept for
/// [`REFRESH`].
pub struct Stars {
    base: String,
    http: reqwest::Client,
    refresh: Duration,
    cache: Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    count: Option<u64>,
    checked: Option<Instant>,
}

#[derive(Deserialize)]
struct Repo {
    stargazers_count: u64,
}

/// The `stargazers_count` of a `GET /repos/{owner}/{repo}` answer.
fn parse(json: &[u8]) -> anyhow::Result<u64> {
    let repo: Repo = serde_json::from_slice(json).context("not a GitHub repository")?;
    Ok(repo.stargazers_count)
}

impl Stars {
    /// `base` is the GitHub API, `https://api.github.com`.
    pub fn new(base: &str) -> Self {
        Self::with_refresh(base, REFRESH)
    }

    fn with_refresh(base: &str, refresh: Duration) -> Self {
        let http = reqwest::Client::builder().timeout(TIMEOUT).user_agent(USER_AGENT).build().expect("client");
        Self { base: base.to_owned(), http, refresh, cache: Mutex::default() }
    }

    /// The star count, or `None` while it was never read. A failed
    /// refresh keeps the last good count; one request asks at a time, and
    /// the others meanwhile get what is kept.
    pub async fn count(&self) -> Option<u64> {
        let due = {
            let mut cache = self.cache.lock().expect("stars cache");
            let due = cache.checked.is_none_or(|at| at.elapsed() >= self.refresh);
            if due {
                cache.checked = Some(Instant::now());
            }
            due
        };
        if due {
            match self.fetch().await {
                Ok(count) => self.cache.lock().expect("stars cache").count = Some(count),
                Err(error) => tracing::warn!(error = format!("{error:#}"), "could not read the GitHub star count"),
            }
        }
        self.cache.lock().expect("stars cache").count
    }

    async fn fetch(&self) -> anyhow::Result<u64> {
        let url = format!("{}/repos/{REPO}", self.base);
        let response = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let body = response.error_for_status()?.bytes().await.with_context(|| format!("GET {url}"))?;
        parse(&body).with_context(|| url)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn the_count_is_read_from_the_repository() {
        let json = br#"{"id": 1, "full_name": "buwiz-bot/buwiz-bot", "stargazers_count": 128, "forks_count": 3}"#;
        assert_eq!(parse(json).unwrap(), 128);
        assert_eq!(parse(br#"{"stargazers_count": 0}"#).unwrap(), 0);
        for bad in [&br#"{"message": "API rate limit exceeded"}"#[..], br#"{"stargazers_count": -1}"#, b"<html>"] {
            assert!(parse(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    /// A stand-in for the GitHub API: answers the repository with what the
    /// test put there, or a 403 like a spent rate limit when it is empty.
    async fn fake_github(answer: Arc<Mutex<Option<String>>>) -> String {
        let app = axum::Router::new().route(
            "/repos/buwiz-bot/buwiz-bot",
            axum::routing::get(move |headers: http::HeaderMap| {
                let body = answer.lock().unwrap().clone();
                async move {
                    use axum::response::IntoResponse;
                    assert_eq!(headers[http::header::USER_AGENT], USER_AGENT);
                    match body {
                        Some(body) => body.into_response(),
                        None => http::StatusCode::FORBIDDEN.into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    #[tokio::test]
    async fn a_failed_refresh_keeps_the_last_good_count() {
        let answer = Arc::new(Mutex::new(None));
        let base = fake_github(answer.clone()).await;
        let stars = Stars::with_refresh(&base, Duration::ZERO);
        assert_eq!(stars.count().await, None, "never read: unknown");

        *answer.lock().unwrap() = Some(r#"{"stargazers_count": 42}"#.into());
        assert_eq!(stars.count().await, Some(42));
        *answer.lock().unwrap() = None;
        assert_eq!(stars.count().await, Some(42), "kept the last good one");
        *answer.lock().unwrap() = Some("{broken".into());
        assert_eq!(stars.count().await, Some(42));
    }

    #[tokio::test]
    async fn the_count_is_asked_for_at_most_every_ten_minutes() {
        let answer = Arc::new(Mutex::new(Some(r#"{"stargazers_count": 7}"#.into())));
        let base = fake_github(answer.clone()).await;
        let stars = Stars::new(&base);
        assert_eq!(stars.count().await, Some(7));
        *answer.lock().unwrap() = Some(r#"{"stargazers_count": 8}"#.into());
        assert_eq!(stars.count().await, Some(7), "still the answer from a moment ago");
    }
}
