//! The sounds Dialo makes itself, as 16-bit mono PCM: the ring of an incoming call and the
//! ringback heard while the other side rings.

pub const RATE: u32 = 16_000;

/// A soft three-note marimba-like phrase, then quiet; repeated while a call rings.
pub fn ringtone() -> Vec<i16> {
    let mut out = vec![0f32; (RATE as f32 * 2.6) as usize];
    // E6, G#6, B6: a bright major triad, struck one after the other.
    for (i, freq) in [1318.5f32, 1661.2, 1975.5].iter().enumerate() {
        let start = (i as f32 * 0.16 * RATE as f32) as usize;
        strike(&mut out, start, *freq, 0.9, 0.30);
    }
    // And again, a little softer.
    for (i, freq) in [1318.5f32, 1661.2, 1975.5].iter().enumerate() {
        let start = ((0.75 + i as f32 * 0.16) * RATE as f32) as usize;
        strike(&mut out, start, *freq, 0.9, 0.22);
    }
    pcm(&out)
}

/// The ringback tone of most of the world outside North America: 425 Hz, 1 s on, 4 s off.
pub fn ringback() -> Vec<i16> {
    let mut out = vec![0f32; RATE as usize * 5];
    let on = RATE as usize;
    for (n, s) in out.iter_mut().take(on).enumerate() {
        let t = n as f32 / RATE as f32;
        // 10 ms ramps, so it doesn't click.
        let ramp = (t / 0.01).min(1.0).min((1.0 - t) / 0.01);
        *s = (t * 425.0 * std::f32::consts::TAU).sin() * 0.18 * ramp;
    }
    pcm(&out)
}

/// A struck bar: the note and a quieter fourth harmonic, both fading away.
fn strike(out: &mut [f32], start: usize, freq: f32, secs: f32, gain: f32) {
    let len = (secs * RATE as f32) as usize;
    for n in 0..len {
        let Some(s) = out.get_mut(start + n) else { break };
        let t = n as f32 / RATE as f32;
        let attack = (t / 0.004).min(1.0);
        let body = (t * freq * std::f32::consts::TAU).sin() * (-t * 5.5).exp();
        let shine = (t * freq * 4.0 * std::f32::consts::TAU).sin() * (-t * 14.0).exp() * 0.25;
        *s += (body + shine) * gain * attack;
    }
}

fn pcm(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn tones_do_not_clip() {
        for tone in [super::ringtone(), super::ringback()] {
            assert!(!tone.is_empty());
            assert!(tone.iter().all(|s| s.unsigned_abs() < i16::MAX as u16));
        }
    }
}
