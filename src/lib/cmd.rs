use std::fs::File;
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;
use nix::sys::signal::{self, Signal};

use crate::lib::job;

use super::config;

async fn remote_cmd(job_id: &Uuid, args: &mut Vec<&str>, output: &mut File, job_registry: &Arc<RwLock<job::JobRegistry>>) -> Result<(), String> {
    let output_file = output.try_clone().unwrap();
    let error_file = output.try_clone().unwrap();
    let cmd_process = Command::new("ssh")
        .arg(config::SEXXI_REMOTE_HOST)
        .args(&*args)
        .stdout(Stdio::from(output_file))
        .stderr(Stdio::from(error_file))
        .spawn();

    match cmd_process {
        Ok(child) => {
            info!("Server process with pid {} is running command `{:?}`", child.id(), args);
            {
                let job_registry = job_registry.read().await;
                let job: Arc<RwLock<job::JobDesc>>;
                if let Some(j) = job_registry.jobs.get(job_id) {
                    job = j.clone();
                    let mut job = job.write().await;
                    job.pid = child.id();
                }
            }

            let result = child.wait_with_output().expect("Ok");
            if !result.status.success() {
                /*
                   Check if Signal::SIGINT was received. If it was the case
                   this is probably because the the job was canceled.
                   exit code 130 is a result of Control-C/SIGINT
                */
                if result.status.to_string() == "exit code: 130" {
                    let job_registry = job_registry.read().await;
                    let job: Arc<RwLock<job::JobDesc>>;
                    if let Some(j) = job_registry.jobs.get(job_id) {
                        job = j.clone();
                        let mut job = job.write().await;
                        job.status = job::JobStatus::Canceled;
                    }
                }
                return Err(format!("remote command failed: {}", result.status));
            }
        },
        Err(e) => {
            return Err(format!("Command `{:?}` failed, server process didn't start: {}", args, e));
        },

    }

    Ok(())
}

async fn remote_git_cmd(job_id: &Uuid, args: &mut Vec<&str>, output: &mut File, job_registry: &Arc<RwLock<job::JobRegistry>>) -> Result<(), String> {
    let mut git_cmd = vec!["git", "-C", config::SEXXI_WORK_TREE];
    git_cmd.append(args);
    remote_cmd(job_id, &mut git_cmd, output, job_registry).await
}

pub async fn remote_git_reset_branch(job_id: &Uuid, output: &mut File, job_registry: &Arc<RwLock<job::JobRegistry>>) -> Result<(), String> {
    let mut cmd = vec!["checkout", "master"];
    remote_git_cmd(job_id, &mut cmd, output, job_registry).await
}

pub async fn remote_git_fetch_upstream(job_id: &Uuid, output: &mut File, job_registry: &Arc<RwLock<job::JobRegistry>>) -> Result<(), String> {
    let mut cmd = vec!["fetch", "--all", "-p"];
    remote_git_cmd(job_id, &mut cmd, output, job_registry).await
}

pub async fn remote_git_checkout_sha(job_id: &Uuid, sha: &str, bot_ref: &str, output: &mut File, job_registry: &Arc<RwLock<job::JobRegistry>>) -> Result<(), String> {
    let mut cmd = vec!["checkout", sha, "-B", bot_ref];
    remote_git_cmd(job_id, &mut cmd, output, job_registry).await
}

pub async fn remote_git_rebase_upstream(job_id: &Uuid, output: &mut File, job_registry: &Arc<RwLock<job::JobRegistry>>) -> Result<(), String> {
    let mut cmd = vec!["rebase", "upstream/master"];
    remote_git_cmd(job_id, &mut cmd, output, job_registry).await
}

pub async fn remote_git_push(job_id: &Uuid, bot_ref: &str, output: &mut File, job_registry: &Arc<RwLock<job::JobRegistry>>) -> Result<(), String> {
    let mut cmd = vec!["push", "origin", bot_ref, "-f"];
    remote_git_cmd(job_id, &mut cmd, output, job_registry).await
}

pub async fn remote_git_delete_branch(job_id: &Uuid, bot_ref: &str, output: &mut File, job_registry: &Arc<RwLock<job::JobRegistry>>) -> Result<(), String> {
    let mut cmd = vec!["branch", "-D", bot_ref];
    remote_git_cmd(job_id, &mut cmd, output, job_registry).await
}

pub async fn remote_test_rust_repo(job_id: &Uuid, output: &mut File, job_registry: &Arc<RwLock<job::JobRegistry>>) -> Result<(), String> {
    let mut cmd = vec![
        "cd",
        config::SEXXI_WORK_TREE,
        ";",
        "./x.py",
        "test",
        "-i",
        "-j32",
    ];
    remote_cmd(job_id, &mut cmd, output, job_registry).await
}
