use std::process::ExitCode;

fn main() -> ExitCode {
    match koelu::cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => koelu::cli::error_exit(error),
    }
}
