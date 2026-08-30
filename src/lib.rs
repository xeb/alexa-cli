pub mod auth;
pub mod avs;
pub mod cache;
pub mod config;
pub mod remote;

// The round-trip voice pipeline and its command tree — see the `speech`
// feature's doc comment in Cargo.toml. `cli` is gated alongside `audio`/
// `stt`/`tts` because it is itself a consumer of all three (the bare-text
// `ask` path and `doctor`'s live round-trip), not because it has its own
// direct dependency on whisper-rs/sherpa-onnx/symphonia/rubato.
#[cfg(feature = "speech")]
pub mod audio;
#[cfg(feature = "speech")]
pub mod cli;
#[cfg(feature = "speech")]
pub mod stt;
#[cfg(feature = "speech")]
pub mod tts;
