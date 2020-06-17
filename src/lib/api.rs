extern crate hyper;

use hyper::{Body, Client, Method, Request, StatusCode};
use hyper_tls::HttpsConnector;
use std::env;
use std::fs::File;
use std::io::prelude::*;

use super::config;

pub const COMMIT_PENDING: &'static str = "pending";
pub const COMMIT_SUCCESS: &'static str = "success";
pub const COMMIT_FAILURE: &'static str = "failure";

pub async fn post_comment(comment: &str, pr_number: u64) -> Result<(), String> {
    let uri = format!("https://api.github.com/repos/sexxi-goose/{}/issues/{}/comments", config::SEXXI_PROJECT, pr_number);
    let body = format!(r#"{{"body": "{}"}}"#, comment);
    if let Err(e) = post(uri, body).await {
        return Err(format!("unable to post comments: {}", e));
    }
    Ok(())
}

pub async fn update_commit_status(sha: &str, status: &str, desc: &str, build_log_url: &str) -> Result<(), String> {
    let uri = format!("https://api.github.com/repos/sexxi-goose/{}/statuses/{}", config::SEXXI_PROJECT, sha);
    let body = format!(r#"{{ "state": "{}", "target_url": "{}", "description": "{}", "context": "CI/SEXXI-BOT" }}"#, status, build_log_url, desc);

    if let Err(e) = post(uri, body).await {
        return Err(format!("unable to update commit status: {}", e));
    }
    Ok(())
}

async fn post(
    uri: String,
    body: String,
    ) -> Result<(), String> {
    match fetch_secret() {
        Ok(secret) => {
            let username_password = format!("{}:{}", config::SEXXI_USERNAME, secret);
            let encoded = base64::encode_config(username_password, base64::STANDARD);
            let auth_header = format!("Basic {}", encoded);
            let req = Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header("Authorization", auth_header)
                .header("User-Agent", "sexxi-bot-automation")
                .body(Body::from(body))
                .expect("ok");

            let connector = HttpsConnector::new();
            let client = Client::builder().build::<_, hyper::Body>(connector);
            let result = client.request(req).await;
            match result {
                Ok(resp) => {
                    match resp.status() {
                        StatusCode::CREATED => Ok(()),
                        _ => Err(format!("http request error: {:#?}", resp)),
                    }
                },
                Err(e) => Err(format!("http request failure: {}", e))
            }
        }
        Err(e) => Err(format!("failed fetch secret for http request: {}", e))
    }
}

fn fetch_secret_content(path: &str) -> Result<String, String> {
    match File::open(path) {
        Ok(mut file) => {
            let mut s = String::new();
            match file.read_to_string(&mut s) {
                Ok(_) => Ok(s),
                Err(e) => Err(format!("failed to read file {}, {}", path, e)),
            }
        }
        Err(e) => Err(format!("unable to open secret file {}: {}", path, e)),
    }
}

// TODO(azhng): cache result
fn fetch_secret() -> Result<String, String> {
    let p = env::var(config::SEXXI_SECRET);
    match p {
        Ok(path) => fetch_secret_content(&path),
        Err(e) => Err(format!(
                "env var {} not properly set: {}",
                config::SEXXI_SECRET,
                e
                )),
    }
}
