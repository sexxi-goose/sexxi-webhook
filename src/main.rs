extern crate base64;
extern crate hyper;
extern crate hyper_tls;
extern crate serde_json;

use std::env;
use std::io::prelude::*;
use std::fs::File;

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Method, Request, Response, Server, StatusCode};
use hyper::body::Buf;
use hyper_tls::HttpsConnector;

const REVIEW_REQUESTED: &'static str = "review_requested";
const REVIEWER: &'static str = "sexxi-bot";

// Should be located in $HOME/.secrets/.gh
const SEXXI_SECRET: &'static str = "SEXXI_SECRET";
const SEXXI_USERNAME: &'static str = "sexxi-bot";

const COMMENT_JOB_START: &'static str = "Start running build job...";

#[derive(Debug)]
struct JobDesc {
    action: String,
    reviewer: String,
    sha: String,
    pr_num: u64,
}

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
    let p = env::var(SEXXI_SECRET);
    match p {
        Ok(path) => fetch_secret_content(&path),
        Err(e) => Err(format!("env var {} not properly set: {}", SEXXI_SECRET, e)),
    }
}


async fn post_comment(comment: String, pr_number: u64) -> Result<(), String> {
    match fetch_secret() {
        Ok(secret) => {
            // TODO(azhng): apparently we can't format string using const literal str.
            let uri = format!("https://api.github.com/repos/sexxi-goose/webhook-test/issues/{}/comments", pr_number);
            let username_password = format!("{}:{}", SEXXI_USERNAME, secret);
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
                    println!("resp: {:#?}", &resp);
                    Ok(())
                },
                Err(e) => Err(format!("http request to comment failed: {}", e))
            }
        }
        Err(e) => Err(format!("failed fetch secret for http request: {}", e))
    }
}

fn bad_req() -> Response<Body> {
    let mut resp = Response::default();
    *resp.status_mut() = StatusCode::from_u16(400).unwrap();
    resp
}

async fn start_build_job(job: JobDesc) {
    if job.reviewer == REVIEWER && job.action == REVIEW_REQUESTED {
        match post_comment(String::from(COMMENT_JOB_START), job.pr_num).await {
            Ok(_) => {}
            Err(e) => {
                println!("Failed to start job for sha {}: {}", &job.sha, e);
                return;
            }
        }
        tokio::spawn(async move {
            println!("Kicking of job for sha: {}", &job.sha);
        });
    } else {
        println!("Webhook rescieved: skipped due to unsatisfied conditions:");
        println!("\tjob: {:#?}", &job);
    }
}

async fn handler(req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
    match (req.method(), req.uri().path()) {
        (&Method::POST, "/github") => {
            let mut body = hyper::body::aggregate::<Request<Body>>(req).await?;
            let bytes = body.to_bytes();
            let blob: Result<serde_json::Value, serde_json::Error> = serde_json::from_slice(&bytes);

            match blob {
                Ok(json) => {
                    let pr_number = json["pull_request"]["number"].as_u64().unwrap();
                    let sha = json["pull_request"]["head"]["sha"].as_str().unwrap();
                    let reviewer = json["requested_reviewer"]["login"].as_str().unwrap();
                    let action = json["action"].as_str().unwrap();

                    let job = JobDesc{
                        action: String::from(action),
                        reviewer: String::from(reviewer),
                        sha: String::from(sha),
                        pr_num: pr_number,
                    };

                    // TODO(azhng): track the jobs somehow.
                    start_build_job(job).await;
                    Ok::<_, hyper::Error>(Response::default())
                }
                Err(e) => {
                    println!("parsing error: {}", e);
                    Ok::<_, hyper::Error>(bad_req())
                }
            }

        }

        _ => {
            Ok::<_, hyper::Error>(bad_req())
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    pretty_env_logger::init();

    let make_svc = make_service_fn(|_| async {
        Ok::<_, hyper::Error>(service_fn(handler))
    });

    let addr = ([127, 0, 0, 1], 55420).into();
    let server = Server::bind(&addr).serve(make_svc);

    println!("Listening to http://{}", addr);

    if let Err(err) = server.await {
        eprintln!("server error: {}", err);
    }

    Ok(())
}
