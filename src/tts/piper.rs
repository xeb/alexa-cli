use crate::tts::TtsBackend;
use anyhow::Result;
pub struct Piper;
impl Piper {
    pub fn new() -> Result<Piper> {
        Ok(Piper)
    }
}
impl TtsBackend for Piper {
    fn synth(&self, _text: &str) -> Result<Vec<i16>> {
        anyhow::bail!("piper not implemented yet")
    }
}
