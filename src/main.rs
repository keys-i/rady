use std::process::ExitCode;

fn main() -> ExitCode {
    match pekin::cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => pekin::cli::error_exit(error),
    }
}
