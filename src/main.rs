extern crate pretty_env_logger;
#[macro_use] extern crate log;
extern crate hyper;
extern crate tokio;
extern crate uuid;
mod lib;

use std::sync::Arc;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Method, Server};
use tokio::task;
use tokio::sync::{
    RwLock,
    mpsc,
};
use uuid::Uuid;
use lib::{
    config,
    job,
    handler,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    pretty_env_logger::init();

    let jobs  = job::JobRegistry::new();
    let sync_jobs = Arc::new(RwLock::new(jobs));
    let curr_job = Arc::new(RwLock::new(job::JobDesc::empty()));

    let (tx, mut rx): (mpsc::Sender<Uuid>, mpsc::Receiver<Uuid>) =
                       mpsc::channel(100);

    let runner_jobs = sync_jobs.clone();
    let runner_curr_job = curr_job.clone();
    let job_runner = task::spawn(async move {
        while let Some(job_id) = rx.recv().await {
            job::process_job(&job_id, runner_jobs.clone(), &runner_curr_job).await;
        }
    });

    let make_svc = make_service_fn(move |_| {
        let svc_jobs = sync_jobs.clone();
        let svc_sender = tx.clone();
        let svc_curr_job = curr_job.clone();
        async move {
            Ok::<_, hyper::Error>(service_fn(move |req| {
                let jobs = svc_jobs.clone();
                let mut sender = svc_sender.clone();
                let svc_curr_job2 = svc_curr_job.clone();
                async move {
                    match (req.method(), req.uri().path()) {
                        (&Method::POST, "/github") => handler::handle_webhook(req, jobs, &mut sender, & svc_curr_job2.clone()).await,
                        (&Method::GET, "/jobs") => handler::handle_jobs(req, jobs).await,
                        _ => Ok::<_, hyper::Error>(handler::gen_response(400))
                    }
                }
            }))
        }
    });

    let addr = ([127, 0, 0, 1], 55420).into();
    let server = Server::bind(&addr).serve(make_svc);

    info!("Listening to http://{}", addr);
    info!(r"
Server Ready. Configuration:
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
