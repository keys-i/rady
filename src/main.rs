use std::process::ExitCode;

fn main() -> ExitCode {
    match rady::cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => rady::cli::error_exit(error),
    }
}
