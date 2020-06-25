extern crate uuid;

use std::env;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::fs::File;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::{api, config, cmd};

#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    Created,
    Running,
    Failed,
    Finished,
    Canceled,
}

#[derive(Debug, Clone)]
pub struct JobDesc {
    pub id: Uuid,
    pub action: String,
    pub reviewer: String,
    pub sha: String,
    pub pr_num: u64,
    pub head_ref: String,
    pub status: JobStatus,
    pub pid: u32,
}

impl JobDesc {
    pub fn new(action: &str, reviewer: &str, sha: &str, pr_num: u64, head_ref: &str) -> JobDesc {
        JobDesc {
            id: Uuid::new_v4(),
            action: String::from(action),
            reviewer: String::from(reviewer),
            sha: String::from(sha),
            pr_num: pr_num,
            head_ref: String::from(head_ref),
            status: JobStatus::Created,
            pid: 0,
        }
    }

    pub fn build_log_url(&self) -> String {
        format!("{}/{}", *config::BUILD_LOG_BASE_URL, &self.id)
    }
}

#[derive(Debug)]
pub struct JobRegistry {
    pub jobs : HashMap<Uuid, Arc<RwLock<JobDesc>>>,
    pub running_jobs : HashMap<String,Uuid>, //head_ref to uuid
}

impl JobRegistry {
    pub fn new() -> JobRegistry {
        JobRegistry {
            jobs: HashMap::new(),
            running_jobs: HashMap::new(),
        }
    }
}

pub async fn process_job(job_id: &Uuid, job_registry: Arc<RwLock<JobRegistry>>) {
    info!("Starting job {}", &job_id);

    let job: Arc<RwLock<JobDesc>>;

    {
        let rw = job_registry.read().await;
        if let Some(j) = rw.jobs.get(&job_id) {
            job = j.clone();
        } else {
            error!("Job info for {} corrupted or missing", &job_id);
            return;
        }
    }

    job.write().await.status = JobStatus::Running;

    if let Err(e) = start_build_job(&job).await {
        error!("job {} failed due to: {}", &job_id, e);
    }

    // We should be removing the job from the running_jobs list when the job is no more running
    info!("Job is no longer running, remove the job from the running_jobs list");
    job_registry.write().await.running_jobs.remove(&job.read().await.head_ref.clone());

}

async fn job_failure_handler<T: std::fmt::Display>(
    msg: &str,
    job: &JobDesc,
    err: T,
    ) -> Result<(), String> {
    let err_msg = format!("❌ Build job {} failed, access build log [here]({}/{}): {}: {}",
    &job.id, *config::BUILD_LOG_BASE_URL, &job.id, msg, err);
    error!("{}", &err_msg);

    if let Err(e) = api::post_comment(&err_msg, job.pr_num).await {
        return Err(format!("unable to post comment for failed job: {}", e));
    }

    if let Err(e) = api::update_commit_status(&job.sha, api::COMMIT_FAILURE, msg, &job.build_log_url()).await {
        return Err(format!("unable to update commit status for failed job: {}", e));
    }

    info!("job_failure_handler exited");
    Ok(())
}

async fn run_and_build(job: &Arc<RwLock<JobDesc>>) -> Result<(), String> {
    // TODO(azhng): figure out how to perform additional cleanup.

    let log_file_name = format!("{}/{}/{}", env::var("HOME").unwrap(), config::SEXXI_LOG_FILE_DIR, &job.read().await.id);
    let log_file_path = Path::new(&log_file_name);
    info!("Creating log file at: {}", &log_file_name);
    let mut log_file = File::create(&log_file_path).unwrap();
    let bot_ref = format!("bot-{}", &job.read().await.head_ref);

    if let Err(e) = api::update_commit_status(&job.read().await.sha, api::COMMIT_PENDING, "Building job started", &job.read().await.build_log_url()).await {
        return Err(format!("unable to update commit status for failed job: {}", e));
    }


    if let Err(e) = cmd::remote_git_reset_branch(&mut log_file).await {
        return job_failure_handler("unable to reset branch", &*job.read().await, e).await;
    }

    if let Err(e) = cmd::remote_git_fetch_upstream(&mut log_file).await {
        return job_failure_handler("unable to fetch upstream", &*job.read().await, e).await;
    }

    if let Err(e) = cmd::remote_git_checkout_sha(&job.read().await.sha, &bot_ref, &mut log_file).await {
        return job_failure_handler("unable to check out commit", &*job.read().await, e).await;
    }

    if let Err(e) = cmd::remote_git_rebase_upstream(&mut log_file).await {
        return job_failure_handler("unable to rebase against upstream", &*job.read().await, e).await;
    }

    // TODO(azhng): make this a runtime decision.
    //info!("Skipping running test for development");
    if job.read().await.status != JobStatus::Canceled {
        let mut succeed = false;
        let mut err = String::from("");
        match cmd::remote_test_rust_repo(&mut log_file).await {
            Ok(test_process) => {
                job.write().await.pid = test_process.id();
                let result = test_process.wait_with_output().expect("Ok");
                if !result.status.success() {
                    if result.status.to_string() == "exit code: 130" {
                        err = format!("job canceled");
                        job.write().await.status = JobStatus::Canceled;
                    } else {
                        err = format!("unit test failed: {}", result.status);
                        job.write().await.status = JobStatus::Failed;
                    }
                } else {
                    job.write().await.status = JobStatus::Finished;
                    succeed = true;
                }
            },
            Err(e) => {
                err = format!("unable to start testing process: {}", e);
            }
        }

        if !succeed {
            cmd::remote_git_reset_branch(&mut log_file).await.expect("Ok");
            cmd::remote_git_delete_branch(&bot_ref, &mut log_file).await.expect("Ok");
            return job_failure_handler("unit test failed", &*job.read().await, err).await;
        }
    }

    if let Err(e) = cmd::remote_git_push(&bot_ref, &mut log_file).await {
        return job_failure_handler("unable to push bot branch", &*job.read().await, e).await;
    }

    if let Err(e) = cmd::remote_git_reset_branch(&mut log_file).await {
        return job_failure_handler("unable to reset branch for clean up", &*job.read().await, e).await;
    }

    if let Err(e) = cmd::remote_git_delete_branch(&bot_ref, &mut log_file).await {
        return job_failure_handler("unable to delete bot branch", &*job.read().await, e).await;
    }

    let msg = format!("{}, access build log [here]({})", config::COMMENT_JOB_DONE, job.read().await.build_log_url());
    info!("{}", &msg);
    if let Err(e) = api::post_comment(&msg, job.read().await.pr_num).await {
        warn!("failed to post comment for job completion: {}", e);
    }

    if let Err(e) = api::update_commit_status(&job.read().await.sha, api::COMMIT_SUCCESS, &msg, &job.read().await.build_log_url()).await {
        return Err(format!("unable to update commit status for failed job: {}", e));
    }


    info!("Ack job finished");
    Ok(())
}

async fn start_build_job(job: &Arc<RwLock<JobDesc>>) -> Result<(), String> {
    {
        let job = &*job.read().await;
        let comment = format!("{}, job id: [{}]({})", config::COMMENT_JOB_START, &job.id, job.build_log_url());
        if let Err(e) = api::post_comment(&comment, job.pr_num).await {
            return Err(format!("failed to post comment to pr {}: {}", &job.pr_num, e));
        }
    }

    run_and_build(job).await
}
