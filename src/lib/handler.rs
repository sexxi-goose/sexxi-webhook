extern crate serde_json;
extern crate hyper;
extern crate tokio;
extern crate uuid;

use std::sync::Arc;
use hyper::{Body, Request, Response, StatusCode};
use hyper::body::Buf;
use tokio::sync::{
    RwLock,
    mpsc,
};
use uuid::Uuid;
use super::{config, job};
use nix::unistd::Pid;
use nix::sys::signal::{self, Signal};

pub fn gen_response(code: u16) -> Response<Body> {
    let mut resp = Response::default();
    *resp.status_mut() = StatusCode::from_u16(code).unwrap();
    resp
}

pub async fn handle_webhook(
    req: Request<Body>,
    job_registry: Arc<RwLock<job::JobRegistry>>,
    sender: &mut mpsc::Sender<Uuid>,
    ) -> Result<Response<Body>, hyper::Error> {
    let mut body = hyper::body::aggregate::<Request<Body>>(req).await?;
    let bytes = body.to_bytes();
    let blob: Result<serde_json::Value, serde_json::Error> = serde_json::from_slice(&bytes);

    match blob {
        Ok(json) => parse_and_handle(json, job_registry, sender).await,
        Err(e) => {
            error!("parsing error: {}", e);
            Ok::<_, hyper::Error>(gen_response(400))
        }
    }
}

pub async fn handle_jobs(
    _req: Request<Body>,
    job_registry: Arc<RwLock<job::JobRegistry>>,
    ) -> Result<Response<Body>, hyper::Error> {
    let registry = job_registry.read().await;
    let jobs = &registry.jobs;
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
            let job = job.read().await;
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

async fn parse_and_handle(
    json: serde_json::Value,
    job_registry: Arc<RwLock<job::JobRegistry>>,
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
                let job = Arc::new(RwLock::new(job));

                {
                    let mut job_registry = job_registry.write().await;
                    job_registry.jobs.insert(job_id.clone(), job.clone());
                    let c_job_id: Uuid;
                    if let Some(c_id) = job_registry.running_jobs.get(head_ref.clone()) {
                        c_job_id = c_id.clone();
                        if let Some(curr_job) = job_registry.jobs.get(&c_job_id) {
                            let curr_job = curr_job.read().await;
                            // TODO(azhng): stop gap solution to avoid accidentally killing server
                            //  process
                            if head_ref == curr_job.head_ref && curr_job.pid != 0 {
                                info!("New job on same head_ref, killing current job pid: {}", &curr_job.pid);
                                // [To-Do] Fix, could potentially kill a process which is already dead
                                signal::kill(Pid::from_raw(curr_job.pid as i32), Signal::SIGINT);
                            }
                        }
                    }

                    job_registry.running_jobs.insert(head_ref.clone().to_string(), job_id.clone());
                }

                if let Err(e) = sender.send(job_id).await {
                    error!("failed to send job id to job runner: {}", e);
                    {
                        let mut job_registry = job_registry.write().await;
                        job_registry.running_jobs.remove(head_ref.clone());
                    }

                    return Ok::<_, hyper::Error>(gen_response(500));
                };
            },
            config::REVIEW_REQUEST_REMOVED => {
                let head_ref = json["pull_request"]["head"]["ref"].as_str().unwrap();
                let mut job_registry = job_registry.write().await;

                let c_job: Arc<RwLock<job::JobDesc>>;
                let c_job_id: Uuid;
                if let Some(c_id) = job_registry.running_jobs.get(head_ref.clone()) {
                    c_job_id = c_id.clone();
                    if let Some(j) = job_registry.jobs.get(&c_job_id) {
                        c_job = j.clone();
                        let curr_job = c_job.read().await;
                        if head_ref == curr_job.head_ref {
                            info!("Request to run job on {} has been removed. Cancelling running job.",head_ref);
                            // [To-Do] Fix, could potentially kill a process which is already dead
                            signal::kill(Pid::from_raw(curr_job.pid as i32), Signal::SIGINT);

                            // Remove cancelled job
                            job_registry.running_jobs.remove(head_ref);
                        }
                    }
                }
            },
            _ => {
                warn!("Action: {} not handled", action);
            }
        }
    }
    Ok::<_, hyper::Error>(Response::default())
}
