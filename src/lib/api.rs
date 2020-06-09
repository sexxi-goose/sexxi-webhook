extern crate hyper;

use std::fs::File;
use std::env;
use std::io::prelude::*;
use hyper::{Body, Client, Method, Request, StatusCode};
use hyper_tls::HttpsConnector;

use super::config;

fn fetch_secret_content(path: &str) -> Result<String, String> {
    match File::open(path) {
        Ok(mut file) => {
            let mut s = String::new();
            match file.read_to_string(&mut s) {
                Ok(_) => Ok(s),
                Err(e) => Err(format!("failed to read file {}, {}", path, e)),
            }
        },
        Err(e) => Err(format!("unable to open secret file {}: {}", path, e)),
    }

}

// TODO(azhng): cache result
fn fetch_secret() -> Result<String, String> {
    let p = env::var(config::SEXXI_SECRET);
    match p {
        Ok(path) => fetch_secret_content(&path),
        Err(e) => Err(format!("env var {} not properly set: {}", config::SEXXI_SECRET, e)),
    }
}


pub async fn post_comment(comment: String, pr_number: u64) -> Result<(), String> {
    match fetch_secret() {
        Ok(secret) => {
            // TODO(azhng): apparently we can't format string using const literal str.
            let uri = format!("https://api.github.com/repos/sexxi-goose/{}/issues/{}/comments", config::SEXXI_PROJECT, pr_number);
            let username_password = format!("{}:{}", config::SEXXI_USERNAME, secret);
            let encoded = base64::encode_config(username_password, base64::STANDARD);
            let auth_header = format!("Basic {}", encoded);
            let req = Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header("Authorization", auth_header)
                .header("User-Agent", "sexxi-bot-automation")
                .body(Body::from(format!(r#"{{"body": "{}"}}"#, comment)))
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

