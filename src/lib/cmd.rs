use std::fs::File;
use std::process::{Command, Stdio};

use super::config;

fn remote_cmd(args: &mut Vec<&str>, output: &mut File) -> Result<(), String> {
    // TODO(azhng): let's finger cross this works.
    let output_file = output.try_clone().unwrap();
    let error_file = output.try_clone().unwrap();
    let cmd = Command::new("ssh")
        .arg(config::SEXXI_REMOTE_HOST)
        .args(args)
        .stdout(Stdio::from(output_file))
        .stderr(Stdio::from(error_file))
        .output()
        .expect("Ok");

    if !cmd.status.success() {
        return Err(format!("remote command failed: {}", cmd.status));
    }

    Ok(())
}

fn remote_git_cmd(args: &mut Vec<&str>, output: &mut File) -> Result<(), String> {
    let mut git_cmd = vec!["git", "-C", config::SEXXI_WORK_TREE];
    git_cmd.append(args);
    remote_cmd(&mut git_cmd, output)
}

pub fn remote_git_reset_branch(output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["checkout", "master"];
    remote_git_cmd(&mut cmd, output)
}

pub fn remote_git_fetch_upstream(output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["fetch", "--all"];
    remote_git_cmd(&mut cmd, output)
}

pub fn remote_git_checkout_sha(sha: &str, bot_ref: &str, output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["checkout", sha, "-b", bot_ref];
    remote_git_cmd(&mut cmd, output)
}

pub fn remote_git_rebase_upstream(output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["rebase", "upstream/master"];
    remote_git_cmd(&mut cmd, output)
}

pub fn remote_git_push(bot_ref: &str, output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["push", "origin", bot_ref, "-f"];
    remote_git_cmd(&mut cmd, output)
}

pub fn remote_git_delete_branch(bot_ref: &str, output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["branch", "-D", bot_ref];
    remote_git_cmd(&mut cmd, output)
}

pub fn remote_test_rust_repo(output: &mut File) -> Result<(), String> {
    let mut cmd = vec![
        "cd",
        config::SEXXI_WORK_TREE,
        ";",
        "./x.py",
        "test",
        "-i",
        "-j32",
    ];
    remote_cmd(&mut cmd, output)
}
