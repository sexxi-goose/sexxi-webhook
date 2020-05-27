extern crate hyper;
extern crate serde_json;

use std::convert::Infallible;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use hyper::body::Buf;

fn bad_req() -> Response<Body> {
    let mut resp = Response::default();
    *resp.status_mut() = StatusCode::from_u16(400).unwrap();
    resp
}

async fn handler(req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
    match (req.method(), req.uri().path()) {
        (&Method::POST, "/github") => {
            let mut body = hyper::body::aggregate(req).await?;
            let bytes = body.to_bytes();
            let blob: Result<serde_json::Value, serde_json::Error> = serde_json::from_slice(&bytes);

            match blob {
                Ok(json) => {
                    println!("action: {:#?}", json["action"]);
                    println!("reviewer: {:#?}", json["requested_reviewer"]["login"]);
                    println!("sha: {:#?}", json["pull_request"]["head"]["sha"]);
                    Ok(Response::default())
                }
                Err(e) => {
                    println!("parsing error: {}", e);
                    Ok(bad_req())
                }
            }

        }

        _ => {
            Ok(bad_req())
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    pretty_env_logger::init();

    let make_svc = make_service_fn(|_conn| {
        async { Ok::<_, Infallible>(service_fn(handler)) }
    });

    let addr = ([127, 0, 0, 1], 55420).into();

    let server = Server::bind(&addr).serve(make_svc);

    println!("Listening to http://{}", addr);

    server.await?;

    Ok(())
}
