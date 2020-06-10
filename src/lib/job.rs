extern crate uuid;

use std::env;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::fs::File;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::{api, config, cmd};

#[derive(Debug, Clone)]
pub enum JobStatus {
    Created,
    Running,
    Failed,
    Finished,
    Canceled,
}

#[derive(Debug)]
pub struct JobDesc {
    pub id: Uuid,
    pub action: String,
    pub reviewer: String,
    pub sha: String,
    pub pr_num: u64,
    pub head_ref: String,
    pub status: JobStatus,
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
        }
    }

    pub fn build_log_url(&self) -> String {
        format!("{}/{}", config::BUILD_LOG_BASE_URL, &self.id)
    }
}

pub type JobRegistry = HashMap<Uuid, Arc<RwLock<JobDesc>>>;

pub async fn process_job(job_id: &Uuid, jobs: Arc<RwLock<JobRegistry>>) {
    info!("Starting job {}", &job_id);

    let mut succeed = false;
    let job: Arc<RwLock<JobDesc>>;

    {
        let rw = jobs.read().await;
        if let Some(j) = rw.get(&job_id) {
            job = j.clone();
        } else {
            error!("Job info for {} corrupted or missing", &job_id);
            return;
        }
    }

    {
        let mut job = job.write().await;
        job.status = JobStatus::Running;
    }

    {
        let job = &*job.read().await;
        if let Err(e) = start_build_job(job).await {
            error!("job {} failed due to: {}", &job_id, e);
        } else {
            succeed = true;
        }
    }

    {
        let mut job = job.write().await;
        if succeed {
            job.status = JobStatus::Finished;
        } else {
            job.status = JobStatus::Failed;
        }
    }
}

async fn job_failure_handler<T: std::fmt::Display>(
    msg: &str,
    job: &JobDesc,
    err: T,
    ) -> Result<(), String> {
    let err_msg = format!("❌ Build job {} failed, access build log [here]({}/{}): {}: {}",
    &job.id, config::BUILD_LOG_BASE_URL, &job.id, msg, err);
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

async fn run_and_build(job: &JobDesc) -> Result<(), String> {
    // TODO(azhng): figure out how to perform additional cleanup.

    let log_file_name = format!("{}/{}/{}", env::var("HOME").unwrap(), config::SEXXI_LOG_FILE_DIR, &job.id);
    let log_file_path = Path::new(&log_file_name);
    info!("Creating log file at: {}", &log_file_name);
    let mut log_file = File::create(&log_file_path).unwrap();
    let bot_ref = format!("bot-{}", &job.head_ref);

    if let Err(e) = api::update_commit_status(&job.sha, api::COMMIT_PENDING, "Building job started", &job.build_log_url()).await {
        return Err(format!("unable to update commit status for failed job: {}", e));
    }


    if let Err(e) = cmd::remote_git_reset_branch(&mut log_file) {
        return job_failure_handler("unable to reset branch", &job, e).await;
    }

    if let Err(e) = cmd::remote_git_fetch_upstream(&mut log_file) {
        return job_failure_handler("unable to fetch upstream", &job, e).await;
    }

    if let Err(e) = cmd::remote_git_checkout_sha(&job.sha, &bot_ref, &mut log_file) {
        return job_failure_handler("unable to check out commit", &job, e).await;
    }

    if let Err(e) = cmd::remote_git_rebase_upstream(&mut log_file) {
        return job_failure_handler("unable to rebase against upstream", &job, e).await;
    }

    // TODO(azhng): make this a runtime decision.
    //info!("Skipping running test for development");
    if let Err(e) = cmd::remote_test_rust_repo(&mut log_file) {
        cmd::remote_git_reset_branch(&mut log_file).expect("Ok");
        cmd::remote_git_delete_branch(&bot_ref, &mut log_file).expect("Ok");
        return job_failure_handler("unit test failed", &job, e).await;
    }

    if let Err(e) = cmd::remote_git_push(&bot_ref, &mut log_file) {
        return job_failure_handler("unable to push bot branch", &job, e).await;
    }

    if let Err(e) = cmd::remote_git_reset_branch(&mut log_file) {
        return job_failure_handler("unable to reset branch for clean up", &job, e).await;
    }

    if let Err(e) = cmd::remote_git_delete_branch(&bot_ref, &mut log_file) {
        return job_failure_handler("unable to delete bot branch", &job, e).await;
    }

    let msg = format!("{}, access build log [here]({})", config::COMMENT_JOB_DONE, job.build_log_url());
    info!("{}", &msg);
    if let Err(e) = api::post_comment(&msg, job.pr_num).await {
        warn!("failed to post comment for job completion: {}", e);
    }

    if let Err(e) = api::update_commit_status(&job.sha, api::COMMIT_SUCCESS, &msg, &job.build_log_url()).await {
        return Err(format!("unable to update commit status for failed job: {}", e));
    }


    info!("Ack job finished");
    Ok(())
}


async fn start_build_job(job: &JobDesc) -> Result<(), String> {
    let comment = format!("{}, job id: {}", config::COMMENT_JOB_START, &job.id);
    if let Err(e) = api::post_comment(&comment, job.pr_num).await {
        return Err(format!("failed to post comment to pr {}: {}", &job.pr_num, e));
    }
    run_and_build(job).await
}
