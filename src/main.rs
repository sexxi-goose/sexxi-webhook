extern crate base64;
extern crate pretty_env_logger;
#[macro_use] extern crate log;
extern crate hyper;
extern crate hyper_tls;
extern crate serde_json;
extern crate tokio;
extern crate uuid;

use std::sync::Arc;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use hyper::body::Buf;
use tokio::task;
use tokio::sync::{
    RwLock,
    mpsc,
};
use uuid::Uuid;

mod lib;
use lib::{
    config,
    job,
};

fn gen_response(code: u16) -> Response<Body> {
    let mut resp = Response::default();
    *resp.status_mut() = StatusCode::from_u16(code).unwrap();
    resp
}

async fn parse_and_handle(
    json: serde_json::Value,
    jobs: Arc<RwLock<job::JobRegistry>>,
    sender: &mut mpsc::Sender<Uuid>,
    ) -> Result<Response<Body>, hyper::Error> {


    // TODO(azhng): the unwrap here can cause panic. This is because we are recieving
    //   other webhooks other than just PR requesting reviews.
    let reviewer = json["requested_reviewer"]["login"].as_str().unwrap();
    let action = json["action"].as_str().unwrap();

    if reviewer == config::REVIEWER {
        match action {
            config::REVIEW_REQUESTED => {
                let pr_number = json["pull_request"]["number"].as_u64().unwrap();
                let sha = json["pull_request"]["head"]["sha"].as_str().unwrap();
                let head_ref = json["pull_request"]["head"]["ref"].as_str().unwrap();

                let job = job::JobDesc::new(&action, &reviewer, &sha, pr_number, &head_ref);
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
    jobs: Arc<RwLock<job::JobRegistry>>,
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
    jobs: Arc<RwLock<job::JobRegistry>>,
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

    let mut jobs  = job::JobRegistry::new();
    let sync_jobs = Arc::new(RwLock::new(jobs));

    let (tx, mut rx): (mpsc::Sender<Uuid>, mpsc::Receiver<Uuid>) =
                       mpsc::channel(100);

    let runner_jobs = sync_jobs.clone();
    let job_runner = task::spawn(async move {
        while let Some(job_id) = rx.recv().await {
            job::process_job(&job_id, runner_jobs.clone()).await;
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
    config::SEXXI_USERNAME, config::SEXXI_WORK_TREE,
    config::SEXXI_GIT_DIR, config::SEXXI_PROJECT, config::SEXXI_REMOTE_HOST,
    config::SEXXI_LOG_FILE_DIR, config::BUILD_LOG_BASE_URL);


    if let (Err(e), _) = tokio::join!(
        server,
        job_runner,
        ) {
        error!("Server error: {}", e);
    }

    Ok(())
}
