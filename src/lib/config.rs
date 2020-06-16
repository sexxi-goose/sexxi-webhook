use std::env;
use lazy_static::lazy_static;

pub const REVIEW_REQUESTED: &'static str = "review_requested";
pub const REVIEWER: &'static str = "sexxi-bot";
// TODO(azhng): const REPO: &'static str = "rust";

// Should be located in $HOME/.secrets/.gh
pub const SEXXI_SECRET: &'static str = "SEXXI_SECRET";
pub const SEXXI_USERNAME: &'static str = "sexxi-bot";
pub const SEXXI_GIT_DIR: &'static str = "$HOME/scratch/sexxi-rust/.git";
pub const SEXXI_WORK_TREE: &'static str = "$HOME/scratch/sexxi-rust";
pub const SEXXI_PROJECT: &'static str = "rust";

// Default testing repo
pub const SEXXI_TEST_PROJECT: &'static str = "sexxi-webhook-test";

pub const SEXXI_REMOTE_HOST: &'static str = "sorbitol";
pub const SEXXI_LOG_FILE_DIR: &'static str = "www/build-logs";

lazy_static! {
    pub static ref MACHINE_USER: String = env::var("USER").unwrap();
    pub static ref BUILD_LOG_BASE_URL: String = format!("https://csclub.uwaterloo.ca/~{}/build-logs", *MACHINE_USER);
}

pub const COMMENT_JOB_START: &'static str = ":running_man: Start running build job";
pub const COMMENT_JOB_DONE: &'static str = "✅ Job Completed";
