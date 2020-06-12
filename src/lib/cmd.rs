use std::fs::File;
use std::process::{Command, Stdio};

use super::config;

fn remote_cmd(args: &mut Vec<&str>, output: &mut File) -> Result<(), String> {
    // TODO(azhng): let's finger cross this works.
    let output_file = output.try_clone().unwrap();
    let error_file = output.try_clone().unwrap();
    let cmd_process = Command::new("ssh")
        .arg(config::SEXXI_REMOTE_HOST)
        .args(args.clone())
        .stdout(Stdio::from(output_file))
        .stderr(Stdio::from(error_file))
        .spawn();

    match cmd_process {
        Ok(child) => {
            println!("Server process with id {} is running command `{:?}`", child.id(), args);

            let result = child.wait_with_output().expect("Ok");
            if !result.status.success() {
                return Err(format!("remote command failed: {}", result.status));
            }
        },
        Err(e) => {
            println!("Command `{:?}` failed, server process didn't start: {}", args, e);
            std::process::exit(1);
        },

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
    let mut cmd = vec!["fetch", "--all", "-p"];
    remote_git_cmd(&mut cmd, output)
}

pub fn remote_git_checkout_sha(sha: &str, bot_ref: &str, output: &mut File) -> Result<(), String> {
    let mut cmd = vec!["checkout", sha, "-B", bot_ref];
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
