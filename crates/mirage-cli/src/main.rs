//! The `mirage` binary: parse the command line and hand it to the library.
//!
//! All logic lives in [`mirage_cli`]; this entry point only parses arguments and
//! awaits [`mirage_cli::run`], which prints results and maps errors to exit
//! codes.
#![forbid(unsafe_code)]

use clap::Parser;

use mirage_cli::Cli;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    mirage_cli::run(cli).await
}
