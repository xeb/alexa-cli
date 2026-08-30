use std::process::ExitCode;

/// The `speech` feature carries `cli` (this binary's whole command tree) and
/// everything it drives (`audio`/`stt`/`tts`) — see the feature's own doc
/// comment in `Cargo.toml`. It is on by default, so a plain `cargo build`/
/// `cargo run` here is unaffected; this only matters to someone building
/// this specific binary target with `--no-default-features` (a consumer
/// linking `alexa_cli` as a library — e.g. `house`'s `default-features =
/// false` — never builds this target at all, so this never affects them).
/// Fails with a clear message naming the fix rather than a bare "cannot find
/// `cli` in `alexa_cli`" compiler error, mirroring the pattern
/// `house::cli::alexa::run_ask` uses for the same shape of problem.
#[cfg(feature = "speech")]
#[tokio::main]
async fn main() -> ExitCode {
    match alexa_cli::cli::run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(feature = "speech"))]
fn main() -> ExitCode {
    eprintln!(
        "error: this build of the `alexa` binary was not compiled with the `speech` feature \
         (it needs the full TTS -> AVS -> Whisper-STT round trip to do anything at all). \
         Rebuild with `cargo build --features speech` — that is also this crate's default, so \
         only an explicit `--no-default-features` build lacks it."
    );
    ExitCode::FAILURE
}
