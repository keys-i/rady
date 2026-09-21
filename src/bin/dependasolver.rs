use std::ffi::OsString;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut arguments = vec![OsString::from("rady"), OsString::from("dependasolve")];
    arguments.extend(std::env::args_os().skip(1));
    match rady::cli::run_from(arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => rady::cli::error_exit(error),
    }
}
