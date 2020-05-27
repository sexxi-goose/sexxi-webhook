extern crate hyper;
extern crate serde_json;

use std::option::Option;

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use hyper::body::Buf;

const REVIEW_REQUESTED: &'static str = "review_requested";
const REVIEWER: &'static str = "sexxi-bot";

#[derive(Debug)]
struct JobDescription {
    sha: String,
}


fn bad_req() -> Response<Body> {
    let mut resp = Response::default();
    *resp.status_mut() = StatusCode::from_u16(400).unwrap();
    resp
}

fn start_build_job(sha: Option<&str>, reviewer: Option<&str>, action: Option<&str>) {
    match (sha, reviewer, action) {
        (Some(sha), Some(reviewer), Some(action)) if reviewer == REVIEWER && action == REVIEW_REQUESTED => {
            let sha = String::from(sha);
            tokio::spawn(async move {
                println!("Kicking of job for sha: {}", sha);
            });
        }
        (Some(sha), Some(reviewer), Some(action)) => {
            println!("Webhook rescieved: skipped due to unsatisfied conditions:");
            println!("\taction: {:#?}", &action);
            println!("\treviewer: {:#?}", &reviewer);
            println!("\tsha: {:#?}", &sha);
        }
        _ => {
            eprintln!("Error");
        }
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
                    let sha = json["pull_request"]["head"]["sha"].as_str();
                    let reviewer = json["requested_reviewer"]["login"].as_str();
                    let action = json["action"].as_str();

                    // TODO(azhng): track the jobs somehow.
                    start_build_job(sha, reviewer, action);
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
