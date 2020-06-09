extern crate base64;
extern crate pretty_env_logger;
#[macro_use] extern crate log;
extern crate hyper;
extern crate hyper_tls;
extern crate serde_json;
extern crate tokio;
extern crate uuid;

use std::env;
use std::path::Path;
use std::process::{Command, Stdio};
use std::io::prelude::*;
use std::fs::File;
use std::sync::Arc;

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Method, Request, Response, Server, StatusCode};
use hyper::body::Buf;
use hyper_tls::HttpsConnector;

use tokio::task;
use tokio::sync::{
    RwLock,
    mpsc,
};

use uuid::Uuid;

mod job;
use job::*;

// TODO(azhng): check repo name.
const REVIEW_REQUESTED: &'static str = "review_requested";
const REVIEWER: &'static str = "sexxi-bot";

// Should be located in $HOME/.secrets/.gh
const SEXXI_SECRET: &'static str = "SEXXI_SECRET";
const SEXXI_USERNAME: &'static str = "sexxi-bot";
const SEXXI_GIT_DIR: &'static str = "$HOME/scratch/sexxi-rust/.git";
const SEXXI_WORK_TREE: &'static str = "$HOME/scratch/sexxi-rust";
const SEXXI_PROJECT: &'static str = "rust";
//const SEXXI_GIT_DIR: &'static str = "$HOME/src/github.com/webhook-test/.git";
//const SEXXI_WORK_TREE: &'static str = "$HOME/src/github.com/webhook-test";
//const SEXXI_PROJECT: &'static str = "webhook-test";

const SEXXI_REMOTE_HOST: &'static str = "sorbitol";
const SEXXI_LOG_FILE_DIR: &'static str = "www/build-logs";

// TODO(azhng): maybe we need to template out the user name here.
const BUILD_LOG_BASE_URL: &'static str = "https://csclub.uwaterloo.ca/~z577zhan/build-logs";

const COMMENT_JOB_START: &'static str = ":running_man: Start running build job";
const COMMENT_JOB_DONE: &'static str = "✅ Job Completed";

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

fn gen_response(code: u16) -> Response<Body> {
    let mut resp = Response::default();
    *resp.status_mut() = StatusCode::from_u16(code).unwrap();
    resp
}


fn remote_cmd(args: &mut Vec<&str>, output: &mut File) -> Result<(), String> {
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
        "-C", SEXXI_WORK_TREE,
    ];
    git_cmd.append(args);
    remote_cmd(&mut git_cmd, output)
}

fn remote_git_reset_branch(output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["checkout", "master"];
    remote_git_cmd(&mut cmd, output)
}

fn remote_git_fetch_upstream(output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["fetch", "--all"];
    remote_git_cmd(&mut cmd, output)
}

fn remote_git_checkout_sha(sha: &str, bot_ref: &str, output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["checkout", sha, "-b", bot_ref];
    remote_git_cmd(&mut cmd, output)
}

fn remote_git_rebase_upstream(output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["rebase", "upstream/master"];
    remote_git_cmd(&mut cmd, output)
}

fn remote_git_push(bot_ref: &str, output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["push", "origin", bot_ref, "-f"];
    remote_git_cmd(&mut cmd, output)
}

fn remote_git_delete_branch(bot_ref: &str, output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["branch", "-D", bot_ref];
    remote_git_cmd(&mut cmd, output)
}

fn remote_test_rust_repo(output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["cd", SEXXI_WORK_TREE, ";", "./x.py", "test", "-i", "-j32"];
    remote_cmd(&mut cmd, output)
}

async fn job_failure_handler<T: std::fmt::Display>(
    msg: &str,
    job: &JobDesc,
    err: T,
    ) -> Result<(), String> {
    let err_msg = format!("❌ Build job {} failed, access build log [here]({}/{}): {}: {}",
    &job.id, BUILD_LOG_BASE_URL, &job.id, msg, err);
    error!("{}", &err_msg);
    if let Err(e) = post_comment(err_msg, job.pr_num).await {
        Err(format!("unable to post comment for failed job: {}", e))
    } else {
        info!("job_failure_handler exited");
        Ok(())
    }
}

async fn run_and_build(job: &JobDesc) -> Result<(), String> {
    // TODO(azhng): figure out how to perform additional cleanup.

    let log_file_name = format!("{}/{}/{}", env::var("HOME").unwrap(), SEXXI_LOG_FILE_DIR, &job.id);
    let log_file_path = Path::new(&log_file_name);
    info!("Creating log file at: {}", &log_file_name);
    let mut log_file = File::create(&log_file_path).unwrap();
    let bot_ref = format!("bot-{}", &job.head_ref);

    if let Err(e) = remote_git_reset_branch(&mut log_file) {
        return job_failure_handler("unable to reset branch", &job, e).await;
    }

    if let Err(e) = remote_git_fetch_upstream(&mut log_file) {
        return job_failure_handler("unable to fetch upstream", &job, e).await;
    }

    if let Err(e) = remote_git_checkout_sha(&job.sha, &bot_ref, &mut log_file) {
        return job_failure_handler("unable to check out commit", &job, e).await;
    }

    if let Err(e) = remote_git_rebase_upstream(&mut log_file) {
        return job_failure_handler("unable to rebase against upstream", &job, e).await;
    }

    // TODO(azhng): make this a runtime decision.
    info!("Skipping running test for development");
    //if let Err(e) = remote_test_rust_repo(&mut log_file) {
    //    remote_git_reset_branch(&mut log_file).expect("Ok");
    //    remote_git_delete_branch(&bot_ref, &mut log_file).expect("Ok");
    //    return job_failure_handler("unit test failed", &job, e).await;
    //}

    if let Err(e) = remote_git_push(&bot_ref, &mut log_file) {
        return job_failure_handler("unable to push bot branch", &job, e).await;
    }

    if let Err(e) = remote_git_reset_branch(&mut log_file) {
        return job_failure_handler("unable to reset branch for clean up", &job, e).await;
    }

    if let Err(e) = remote_git_delete_branch(&bot_ref, &mut log_file) {
        return job_failure_handler("unable to delete bot branch", &job, e).await;
    }

    let msg = format!("{}, access build log [here]({}/{})", COMMENT_JOB_DONE, BUILD_LOG_BASE_URL, &job.id);
    info!("{}", &msg);
    if let Err(e) = post_comment(msg, job.pr_num).await {
        warn!("failed to post comment for job completion: {}", e);
    } else {
        info!("Ack job finished");
    }
    Ok(())
}


async fn start_build_job(job: &JobDesc) -> Result<(), String> {
    let comment = format!("{}, job id: {}", COMMENT_JOB_START, &job.id);
    if let Err(e) = post_comment(comment, job.pr_num).await {
        return Err(format!("failed to post comment to pr {}: {}", &job.pr_num, e));
    }
    run_and_build(job).await
}

async fn parse_and_handle(
    json: serde_json::Value,
    jobs: Arc<RwLock<JobRegistry>>,
    sender: &mut mpsc::Sender<Uuid>,
    ) -> Result<Response<Body>, hyper::Error> {


    // TODO(azhng): the unwrap here can cause panic. This is because we are recieving
    //   other webhooks other than just PR requesting reviews.
    let reviewer = json["requested_reviewer"]["login"].as_str().unwrap();
    let action = json["action"].as_str().unwrap();

    if reviewer == REVIEWER {
        match action {
            REVIEW_REQUESTED => {
                let pr_number = json["pull_request"]["number"].as_u64().unwrap();
                let sha = json["pull_request"]["head"]["sha"].as_str().unwrap();
                let head_ref = json["pull_request"]["head"]["ref"].as_str().unwrap();

                let job = JobDesc::new(&action, &reviewer, &sha, pr_number, &head_ref);
                let job_id  = job.id.clone();

                let mut jobs = jobs.write().await;
                jobs.insert(job_id.clone(), job);

                if let Err(e) = sender.send(job_id).await {
                    error!("failed to send job id to job runner: {}", e);
                    return Ok::<_, hyper::Error>(gen_response(500));
                };
            },
            _ => {
                warn!("Action: {} not handled", action);
            }
        }
    }
    Ok::<_, hyper::Error>(Response::default())
}

async fn handle_webhook(
    req: Request<Body>,
    jobs: Arc<RwLock<JobRegistry>>,
    sender: &mut mpsc::Sender<Uuid>,
    ) -> Result<Response<Body>, hyper::Error> {
    let mut body = hyper::body::aggregate::<Request<Body>>(req).await?;
    let bytes = body.to_bytes();
    let blob: Result<serde_json::Value, serde_json::Error> = serde_json::from_slice(&bytes);

    match blob {
        Ok(json) => parse_and_handle(json, jobs, sender).await,
        Err(e) => {
            error!("parsing error: {}", e);
            Ok::<_, hyper::Error>(gen_response(400))
        }
    }
}

async fn handle_jobs(
    _req: Request<Body>,
    jobs: Arc<RwLock<JobRegistry>>,
    ) -> Result<Response<Body>, hyper::Error> {
    let jobs = &*jobs.read().await;
    let mut output = String::new();

    output.push_str("<table style=\"width:100%;border:1px solid black;margin-left:auto;margin-right:auto;\">");
    {
        output.push_str("<tr>");
        {
            output.push_str("<th>Job ID</th>");
            output.push_str("<th>Action</th>");
            output.push_str("<th>Reviewer</th>");
            output.push_str("<th>SHA</th>");
            output.push_str("<th>PR Number</th>");
            output.push_str("<th>Head Ref</th>");
            output.push_str("<th>Status</th>");
        }
        output.push_str("</tr>");

        for (_, job) in jobs {
            output.push_str("<tr>");
            {
                // TODO(azhng): hyper link this.
                output.push_str(&format!("<td>{}</td>", job.id));
                output.push_str(&format!("<td>{}</td>", job.action));
                output.push_str(&format!("<td>{}</td>", job.reviewer));
                output.push_str(&format!("<td>{}</td>", job.sha));
                output.push_str(&format!("<td>{}</td>", job.pr_num));
                output.push_str(&format!("<td>{}</td>", job.head_ref));
                output.push_str(&format!("<td>{:?}</td>", job.status));
            }
            output.push_str("</tr>");
        }

    }
    output.push_str("</table>");

    match Response::builder()
        .status(200)
        .header("Content-Type", "text/html")
        .body(Body::from(output)) {
            Ok(resp) => Ok(resp),
            Err(e) => {
                error!("internal error on job query: {}", e);
                Ok::<_, hyper::Error>(gen_response(500))
            }
        }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    pretty_env_logger::init();

    let mut jobs  = JobRegistry::new();
    let sync_jobs = Arc::new(RwLock::new(jobs));

    let (tx, mut rx): (mpsc::Sender<Uuid>, mpsc::Receiver<Uuid>) =
                       mpsc::channel(100);

    let runner_jobs = sync_jobs.clone();
    let job_runner = task::spawn(async move {
        while let Some(job_id) = rx.recv().await {
            info!("Starting job {}", &job_id);

            let mut succeed = false;

            // We break these into separate blocks is to avoid lock contention.
            // we need a write lock here but start_build_job is very expensive.
            // this can blocks our HTTP endpoint for /jobs. Therefore when we
            // start running the build job we release the write lock and acquire
            // and read lock so that other HTTP endpoint won't be blocked.
            // TODO(azhng): write a macro for this. This can be easily genealized.
            {
                let mut rw = runner_jobs.write().await;
                if let Some(job) = rw.get_mut(&job_id) {
                    job.status = JobStatus::Running;
                } else {
                    error!("Job info for {} corrupted or missing", &job_id)
                }
            }

            {
                let rw = runner_jobs.read().await;
                if let Some(job) = rw.get(&job_id) {
                    if let Err(e) = start_build_job(job).await {
                        error!("job {} failed due to: {}", &job_id, e);
                    } else {
                        succeed = true;
                    }
                } else {
                    error!("Job info for {} corrupted or missing", &job_id)
                }
            }

            {
                let mut rw = runner_jobs.write().await;
                if let Some(job) = rw.get_mut(&job_id) {
                    if succeed {
                        job.status = JobStatus::Finished;
                    } else {
                        job.status = JobStatus::Failed;
                    }
                } else {
                    error!("Job info for {} corrupted or missing", &job_id)
                }
            }
        }
    });

    let make_svc = make_service_fn(move |_| {
        let svc_jobs = sync_jobs.clone();
        let svc_sender = tx.clone();
        async move {
            Ok::<_, hyper::Error>(service_fn(move |req| {
                let jobs = svc_jobs.clone();
                let mut sender = svc_sender.clone();
                async move {
                    match (req.method(), req.uri().path()) {
                        (&Method::POST, "/github") => handle_webhook(req, jobs, &mut sender).await,
                        (&Method::GET, "/jobs") => handle_jobs(req, jobs).await,
                        _ => Ok::<_, hyper::Error>(gen_response(400))
                    }
                }
            }))
        }
    });

    let addr = ([127, 0, 0, 1], 55420).into();
    let server = Server::bind(&addr).serve(make_svc);

    info!("Listening to http://{}", addr);
    info!(r"Server Ready. Configuration:
    SEXXI_USERNAME: {},
    SEXXI_WORK_TREE: {},
    SEXXI_GIT_DIR: {},
    SEXXI_PROJECT: {},
    SEXXI_REMOTE_HOST: {},
    SEXXI_LOG_FILE_DIR: {},
    BUILD_LOG_BASE_URL: {}",
    SEXXI_USERNAME, SEXXI_WORK_TREE,
    SEXXI_GIT_DIR, SEXXI_PROJECT, SEXXI_REMOTE_HOST,
    SEXXI_LOG_FILE_DIR, BUILD_LOG_BASE_URL);


    if let (Err(e), _) = tokio::join!(
        server,
        job_runner,
        ) {
        error!("Server error: {}", e);
    }

    Ok(())
}
