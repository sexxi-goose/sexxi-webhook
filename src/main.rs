extern crate base64;
extern crate hyper;
extern crate hyper_tls;
extern crate serde_json;
extern crate uuid;

use std::env;
use std::path::Path;
use std::process::{Command, Stdio};
use std::io::prelude::*;
use std::fs::File;

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Method, Request, Response, Server, StatusCode};
use hyper::body::Buf;
use hyper_tls::HttpsConnector;

use uuid::Uuid;

// TODO(azhng): check repo name.
const REVIEW_REQUESTED: &'static str = "review_requested";
const REVIEWER: &'static str = "sexxi-bot";

// Should be located in $HOME/.secrets/.gh
const SEXXI_SECRET: &'static str = "SEXXI_SECRET";
const SEXXI_USERNAME: &'static str = "sexxi-bot";
//const SEXXI_GIT_DIR: &'static str = "$HOME/scratch/sexxi-rust/.git";
//const SEXXI_WORK_TREE: &'static str = "$HOME/scratch/sexxi-rust";
//const SEXXI_PROJECT: &'static str = "rust";
const SEXXI_GIT_DIR: &'static str = "$HOME/src/github.com/webhook-test/.git";
const SEXXI_WORK_TREE: &'static str = "$HOME/src/github.com/webhook-test";
const SEXXI_PROJECT: &'static str = "webhook-test";

const SEXXI_REMOTE_HOST: &'static str = "sorbitol";
const SEXXI_LOG_FILE_DIR: &'static str = "www/build-logs";

const BUILD_LOG_BASE_URL: &'static str = "https://csclub.uwaterloo.ca/~z577zhan/build-logs";

const COMMENT_JOB_START: &'static str = "Start running build job...";
const COMMENT_JOB_DONE: &'static str = "Job Completed";

#[derive(Debug)]
struct JobDesc {
    action: String,
    reviewer: String,
    sha: String,
    pr_num: u64,
    head_ref: String,
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
            let uri = format!("https://api.github.com/repos/sexxi-goose/{}/issues/{}/comments", SEXXI_PROJECT, pr_number);
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
                Ok(_resp) => {
                    //println!("resp: {:#?}", &resp);
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


fn remote_cmd(args: Vec<&str>, output: &mut File) -> Result<(), String> {
    // TODO(azhng): let's finger cross this works.
    let output_file = output.try_clone().unwrap();
    let error_file = output.try_clone().unwrap();
    let cmd = Command::new("ssh")
        .arg(SEXXI_REMOTE_HOST)
        .args(args)
        .stdout(Stdio::from(output_file))
        .stderr(Stdio::from(error_file))
        .output()
        .expect("Ok");

    if !cmd.status.success() {
        return Err(format!("remote command failed: {}", cmd.status));
    }

    Ok(())
}

fn remote_git_cmd(args: &mut Vec<&str>, output: &mut File) -> Result<(), String> {
    let mut git_cmd = vec![
        "git",
        "--git-dir", SEXXI_GIT_DIR,
        "--work-tree", SEXXI_WORK_TREE,
    ];
    git_cmd.append(args);
    remote_cmd(git_cmd, output)
}

async fn job_failure_handler<T: std::fmt::Display>(msg: &str, job: &JobDesc, log_file_path: &str, err: T) {
    // TODO(azhng): format the buid log url
    let err_msg = format!("job failed for sha {}, log file path {:#?}: {}: {}", &job.sha, log_file_path, msg, err);
    println!("{}", &err_msg);
    post_comment(err_msg, job.pr_num).await.expect("Ok");
    println!("Ack job finished");
}

fn remote_git_reset_branch(output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["checkout", "master"];
    remote_git_cmd(&mut cmd, output)
}

fn remote_git_fetch_upstream(output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["fetch", "-a"];
    remote_git_cmd(&mut cmd, output)
}

fn remote_git_checkout_sha(sha: &str, bot_ref: &str, output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["checkout", sha, "-b", bot_ref];
    remote_git_cmd(&mut cmd, output)
}

// TODO(azhng): ...
fn remote_git_rebase(sha: &str) {}

fn remote_git_push(bot_ref: &str, output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["push", "origin", bot_ref, "-f"];
    remote_git_cmd(&mut cmd, output)
}

fn remote_git_delete_branch(bot_ref: &str, output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["branch", "-D", bot_ref];
    remote_git_cmd(&mut cmd, output)
}


async fn run_and_build(job: JobDesc) {
    let job_id = Uuid::new_v4();
    let log_file_name = format!("{}/{}/{}", env::var("HOME").unwrap(), SEXXI_LOG_FILE_DIR, &job_id);
    let log_file_path = Path::new(&log_file_name);
    println!("Creating log file at: {}", &log_file_name);
    let mut log_file = File::create(&log_file_path).unwrap();
    let bot_ref = format!("bot-{}", &job.head_ref);

    if let Err(e) = remote_git_reset_branch(&mut log_file) {
        job_failure_handler("unable to reset branch", &job, &*log_file_name, e).await;
        return;
    }

    if let Err(e) = remote_git_fetch_upstream(&mut log_file) {
        job_failure_handler("unable to fetch upstream", &job, &log_file_name, e).await;
        return;
    }

    if let Err(e) = remote_git_checkout_sha(&job.sha, &bot_ref, &mut log_file) {
        job_failure_handler("unable to check out sha", &job, &log_file_name, e).await;
        return;
    }

    // TODO(azhng): run test

    if let Err(e) = remote_git_push(&bot_ref, &mut log_file) {
        job_failure_handler("unable to push bot branch", &job, &log_file_name, e).await;
        return;
    }

    if let Err(e) = remote_git_reset_branch(&mut log_file) {
        job_failure_handler("unable to reset branch for clean up", &job, &log_file_name, e).await;
        return;
    }

    if let Err(e) = remote_git_delete_branch(&bot_ref, &mut log_file) {
        job_failure_handler("unable to delete bot branch", &job, &log_file_name, e).await;
        return;
    }

    let msg = format!("{}, access build log [here]({}/{})",
    COMMENT_JOB_DONE, BUILD_LOG_BASE_URL, &job_id);
    println!("{}", &msg);
    post_comment(msg, job.pr_num).await.unwrap();
    println!("Ack job finished");
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
            run_and_build(job).await;
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
                    let head_ref = json["pull_request"]["head"]["ref"].as_str().unwrap();
                    let action = json["action"].as_str().unwrap();

                    let job = JobDesc{
                        action: String::from(action),
                        reviewer: String::from(reviewer),
                        sha: String::from(sha),
                        pr_num: pr_number,
                        head_ref: String::from(head_ref),
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

    println!(r"Server Ready. Configuration:
    SEXXI_USERNAME: {},
    SEXXI_WORK_TREE: {},
    SEXXI_GIT_DIR: {},
    SEXXI_PROJECT: {},
    SEXXI_REMOTE_HOST: {},
    SEXXI_LOG_FILE_DIR: {},
    BUILD_LOG_BASE_URL: {}", SEXXI_USERNAME, SEXXI_WORK_TREE,
    SEXXI_GIT_DIR, SEXXI_PROJECT, SEXXI_REMOTE_HOST,
    SEXXI_LOG_FILE_DIR, BUILD_LOG_BASE_URL);

    if let Err(err) = server.await {
        eprintln!("server error: {}", err);
    }

    Ok(())
}
