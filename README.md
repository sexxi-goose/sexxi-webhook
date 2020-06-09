# SE'XXI-Webhook

A CI job server built using Rust that runs rust-lang test suite using CSC machines.
It runs on the `caffeine` node since it is the only CSC machines exposed to the network.
It then use other CSC machines to run build/test jobs.

The job server caches build log file in a public static file server and the job queue
information can be accessed through `/jobs` endpoint.

# Assumption

The job server assumes that the rust-lang repo is stored in `$HOME/scratch/sexxi-rust` and
`origin` remote is pointed to the `sexxi-goose/rust` and `upstream` remote is pointed to
`rust-lang/rust`.

It also assumes it runs in CSC environment so `$HOME/www/` directory exists for storing
build log files.

# Build

``` bash
cargo build
```


# Run

``` bash
SEXXI_SECRET=/path/to/github/api/token RUST_LOG=info cargo run
```
