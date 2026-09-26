use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!(
        "Rady moved to Pekin. Install it with `cargo install pekin --locked`, then run `pekin --help`."
    );
    ExitCode::FAILURE
}
