use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!(
        "Rady moved to Koelu. Install it with `cargo install koelu --locked`, then run `koelu --help`."
    );
    ExitCode::FAILURE
}
