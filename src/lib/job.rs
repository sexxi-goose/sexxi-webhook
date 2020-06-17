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

    let mut succeed = false;
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

    {
        let mut job = job.write().await;
        job.status = JobStatus::Running;
    }

    let copy_job : JobDesc;
    {
        let job = &*job.read().await;

        /*
           Important: We can't pass job directly into start_build_job, otherwise we will
           have a deadlock. We want to update the JobDesc for the job with the PID of the
           running process, but since we have the read lock, we can't acquire the write lock.

           If we want to avoid having to request the read lock everytime we need it, we should
           duplicate the current state (info) of the JobDesc. We may want to change this later
           if we change more than the PID info while running the job.
        */
        copy_job = job.clone();
    }

    if let Err(e) = start_build_job(&copy_job, &job_registry).await {
        error!("job {} failed due to: {}", &job_id, e);
    } else {
        succeed = true;
    }

    {
        let mut job = job.write().await;

        /*
           We don't want to remove the head_ref key from the cancelled
           job if the JobStatus is Canceled because when a job is Canceled,
           a new job is started with the same key. ie: removing the key results
           in removing a running job not the canceled job.
        */
        if (job.status != JobStatus::Canceled) {
            {
                let mut job_registry = job_registry.write().await;
                job_registry.running_jobs.remove(&job.head_ref.clone());
            }
        }

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

async fn run_and_build(job: &JobDesc, job_registry: &Arc<RwLock<JobRegistry>>) -> Result<(), String> {
    // TODO(azhng): figure out how to perform additional cleanup.

    let log_file_name = format!("{}/{}/{}", env::var("HOME").unwrap(), config::SEXXI_LOG_FILE_DIR, &job.id);
    let log_file_path = Path::new(&log_file_name);
    info!("Creating log file at: {}", &log_file_name);
    let mut log_file = File::create(&log_file_path).unwrap();
    let bot_ref = format!("bot-{}", &job.head_ref);

    if let Err(e) = api::update_commit_status(&job.sha, api::COMMIT_PENDING, "Building job started", &job.build_log_url()).await {
        return Err(format!("unable to update commit status for failed job: {}", e));
    }


    if let Err(e) = cmd::remote_git_reset_branch(&job.id, &mut log_file, job_registry).await {
        return job_failure_handler("unable to reset branch", &job, e).await;
    }

    if let Err(e) = cmd::remote_git_fetch_upstream(&job.id, &mut log_file, job_registry).await {
        return job_failure_handler("unable to fetch upstream", &job, e).await;
    }

    if let Err(e) = cmd::remote_git_checkout_sha(&job.id, &job.sha, &bot_ref, &mut log_file, job_registry).await {
        return job_failure_handler("unable to check out commit", &job, e).await;
    }

    if let Err(e) = cmd::remote_git_rebase_upstream(&job.id, &mut log_file, job_registry).await {
        return job_failure_handler("unable to rebase against upstream", &job, e).await;
    }

    // TODO(azhng): make this a runtime decision.
    //info!("Skipping running test for development");
    if let Err(e) = cmd::remote_test_rust_repo(&job.id, &mut log_file, job_registry).await {
        cmd::remote_git_reset_branch(&job.id, &mut log_file, job_registry).await.expect("Ok");
        cmd::remote_git_delete_branch(&job.id, &bot_ref, &mut log_file, job_registry).await.expect("Ok");
        return job_failure_handler("unit test failed", &job, e).await;
    }

    if let Err(e) = cmd::remote_git_push(&job.id, &bot_ref, &mut log_file, job_registry).await {
        return job_failure_handler("unable to push bot branch", &job, e).await;
    }

    if let Err(e) = cmd::remote_git_reset_branch(&job.id, &mut log_file, job_registry).await {
        return job_failure_handler("unable to reset branch for clean up", &job, e).await;
    }

    if let Err(e) = cmd::remote_git_delete_branch(&job.id, &bot_ref, &mut log_file, job_registry).await {
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


async fn start_build_job(job: &JobDesc, job_registry: &Arc<RwLock<JobRegistry>>) -> Result<(), String> {
    let comment = format!("{}, job id: {}", config::COMMENT_JOB_START, &job.id);
    if let Err(e) = api::post_comment(&comment, job.pr_num).await {
        return Err(format!("failed to post comment to pr {}: {}", &job.pr_num, e));
    }
    run_and_build(job, job_registry).await
}
