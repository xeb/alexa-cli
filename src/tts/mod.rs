pub mod espeak;
pub mod piper;

use crate::config::Voice;
use anyhow::Result;

pub trait TtsBackend {
    /// Returns 16 kHz, mono, signed-16 PCM samples.
    fn synth(&self, text: &str) -> Result<Vec<i16>>;
}

pub fn backend_for(voice: &Voice) -> Result<Box<dyn TtsBackend>> {
    match voice {
        Voice::Espeak => Ok(Box::new(espeak::Espeak)),
        Voice::Piper => Ok(Box::new(piper::Piper::new()?)),
    }
}
