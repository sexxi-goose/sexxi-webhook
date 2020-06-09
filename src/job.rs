extern crate uuid;

use std::collections::HashMap;
use uuid::Uuid;

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
}

pub type JobRegistry = HashMap<Uuid, JobDesc>;
