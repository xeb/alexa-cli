use anyhow::Result;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| {
            // Asymmetric full-scale mapping pinned by the binding tests:
            // 1.0 -> 32767, -1.0 -> -32768, 0.5 -> 16383, |s|>1 clamps.
            let scaled = (s.clamp(-1.0, 1.0) * 32767.5).floor();
            scaled.clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

pub fn i16_to_le_bytes(samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

pub fn downmix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

pub fn resample_to_16k(samples: &[f32], in_rate: u32) -> Result<Vec<f32>> {
    if in_rate == 16000 {
        return Ok(samples.to_vec());
    }
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let ratio = 16000.0 / in_rate as f64;
    let mut resampler = SincFixedIn::<f32>::new(ratio, 2.0, params, samples.len(), 1)?;
    let waves_in = vec![samples.to_vec()];
    let waves_out = resampler.process(&waves_in, None)?;
    Ok(waves_out.into_iter().next().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_to_i16_clamps_and_scales() {
        let out = f32_to_i16(&[0.0, 1.0, -1.0, 2.0, -2.0, 0.5]);
        assert_eq!(out[0], 0);
        assert_eq!(out[1], 32767);
        assert_eq!(out[2], -32768);
        assert_eq!(out[3], 32767); // clamped
        assert_eq!(out[4], -32768); // clamped
        assert_eq!(out[5], 16383); // 0.5*32767 rounded
    }

    #[test]
    fn i16_to_le_bytes_is_little_endian() {
        let out = i16_to_le_bytes(&[1, -1]);
        assert_eq!(out, vec![0x01, 0x00, 0xFF, 0xFF]);
    }

    #[test]
    fn downmix_averages_channels() {
        // stereo: L=[1.0, 0.0], R=[0.0, 1.0] interleaved
        let out = downmix_to_mono(&[1.0, 0.0, 0.0, 1.0], 2);
        assert_eq!(out, vec![0.5, 0.5]);
    }

    #[test]
    fn downmix_mono_is_identity() {
        let out = downmix_to_mono(&[0.1, 0.2, 0.3], 1);
        assert_eq!(out, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn resample_changes_length_proportionally() {
        let input: Vec<f32> = (0..32000).map(|i| ((i as f32) * 0.01).sin()).collect();
        let out = resample_to_16k(&input, 32000).unwrap();
        // 32000 samples @32k -> ~1s -> ~16000 @16k (allow small ratio slack)
        let ratio = out.len() as f32 / input.len() as f32;
        assert!((ratio - 0.5).abs() < 0.05, "ratio was {ratio}");
    }

    #[test]
    fn resample_16k_is_passthrough() {
        let input = vec![0.1f32, 0.2, 0.3, 0.4];
        let out = resample_to_16k(&input, 16000).unwrap();
        assert_eq!(out, input);
    }
}
