//! Finds a track's tempo and beat grid on the user's machine, so the video's cuts land on the
//! beat. FFmpeg decodes the file; the rest is plain arithmetic (no model, no cloud call).
//!
//! The method is the usual one for steady-tempo music: an onset envelope (how sharply the loudness
//! rises, in the whole signal and in the bass), its autocorrelation to find the beat period, and
//! a comb search to line the grid up with the onsets. Generated songs keep a steady tempo, which
//! is what this assumes.

use std::path::Path;
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::backend::video;

/// Decoding rate: enough for onsets, and a three-minute track is only two million samples.
const SAMPLE_RATE: u32 = 11_025;
/// Samples between envelope frames (about 86 frames a second).
const HOP: usize = 128;
const WINDOW: usize = 256;
/// Tempos considered, in beats a minute.
const MIN_BPM: f64 = 70.0;
const MAX_BPM: f64 = 190.0;
/// The tempo prior's centre; an octave either side scores about 60% as well.
const PREFERRED_BPM: f64 = 125.0;
/// The bass band's corner frequency (kick drums sit below it).
const BASS_HZ: f32 = 150.0;
const BASS_WEIGHT: f32 = 1.5;
/// How far around the autocorrelation's period the comb search looks, and in what steps.
const REFINE_SPAN: f64 = 0.03;
const REFINE_STEPS: i32 = 120;
const BEATS_PER_BAR: usize = 4;

/// A track's beats.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BeatGrid {
  pub bpm: f64,
  /// Beat times in seconds, from the first beat to the end of the track.
  pub beats: Vec<f64>,
  /// Which beat in `beats` starts a bar (the first downbeat); bars are four beats.
  pub downbeat: usize,
  /// The track's length in seconds.
  pub duration: f64,
}

impl BeatGrid {
  /// Seconds between beats.
  pub fn period(&self) -> f64 {
    if self.bpm > 0.0 {
      60.0 / self.bpm
    } else {
      0.0
    }
  }

  /// A plain grid at `bpm` for a track of `duration` seconds, for when analysis isn't possible.
  pub fn steady(bpm: f64, duration: f64) -> Self {
    let period = 60.0 / bpm;
    let beats = (0..).map(|i| f64::from(i) * period).take_while(|t| *t < duration).collect();
    Self { bpm, beats, downbeat: 0, duration }
  }
}

/// Decodes `path` and finds its beats. Blocking: run it off the UI thread.
pub fn analyse_file(path: &Path) -> anyhow::Result<BeatGrid> {
  let samples = decode(path)?;
  anyhow::ensure!(!samples.is_empty(), "The track has no audio to analyse");
  analyse(&samples, SAMPLE_RATE).ok_or_else(|| anyhow::anyhow!("Couldn't find a steady beat in the track"))
}

/// The beat grid of mono `samples` at `rate` Hz; `None` for silence or clips too short to tell.
pub fn analyse(samples: &[f32], rate: u32) -> Option<BeatGrid> {
  let duration = samples.len() as f64 / f64::from(rate);
  let fps = f64::from(rate) / HOP as f64;
  let (envelope, bass) = onset_envelopes(samples, rate);
  // Two bars at the slowest tempo is the least that says anything.
  if (envelope.len() as f64) < fps * 8.0 * 60.0 / MAX_BPM || envelope.iter().all(|v| *v <= f32::EPSILON) {
    return None;
  }
  let rough = rough_period(&envelope, fps)?;
  let (period, phase) = refine(&envelope, rough);
  let seconds_per_frame = 1.0 / fps;
  let beats: Vec<f64> = (0..).map(|i| (phase + f64::from(i) * period) * seconds_per_frame).take_while(|t| *t < duration).collect();
  if beats.len() < BEATS_PER_BAR {
    return None;
  }
  let downbeat = downbeat(&bass, &beats, fps);
  Some(BeatGrid { bpm: 60.0 * fps / period, beats, downbeat, duration })
}

/// Mono 32-bit float samples at [`SAMPLE_RATE`], via FFmpeg.
fn decode(path: &Path) -> anyhow::Result<Vec<f32>> {
  anyhow::ensure!(video::ffmpeg_available(), "Finding the beat needs FFmpeg: install it with `brew install ffmpeg`.");
  let output = video::command("ffmpeg").args(["-v", "error", "-i"]).arg(path).args(["-vn", "-ac", "1", "-ar", &SAMPLE_RATE.to_string(), "-f", "f32le", "-"]).stdout(Stdio::piped()).stderr(Stdio::piped()).output()?;
  anyhow::ensure!(output.status.success(), "FFmpeg couldn't read the track: {}", String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or("unknown error"));
  Ok(output.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

/// How sharply the loudness rises at each frame: the combined envelope (whole signal plus
/// weighted bass) used for tempo, and the bass alone used to find bar starts.
fn onset_envelopes(samples: &[f32], rate: u32) -> (Vec<f32>, Vec<f32>) {
  let alpha = 1.0 - (-2.0 * std::f32::consts::PI * BASS_HZ / rate as f32).exp();
  let mut low = 0.0f32;
  let lows: Vec<f32> = samples
    .iter()
    .map(|s| {
      low += alpha * (s - low);
      low
    })
    .collect();
  let rise = |signal: &[f32]| -> Vec<f32> {
    let frames = signal.len().saturating_sub(WINDOW) / HOP;
    let loudness: Vec<f32> = (0..frames)
      .map(|f| {
        let window = &signal[f * HOP..f * HOP + WINDOW];
        let rms = (window.iter().map(|s| s * s).sum::<f32>() / WINDOW as f32).sqrt();
        (1.0 + 1000.0 * rms).ln()
      })
      .collect();
    let mut out = vec![0.0; frames];
    for f in 1..frames {
      out[f] = (loudness[f] - loudness[f - 1]).max(0.0);
    }
    out
  };
  let (full, bass) = (rise(samples), rise(&lows));
  let combined = full.iter().zip(&bass).map(|(f, b)| f + BASS_WEIGHT * b).collect();
  (combined, bass)
}

/// The beat period in frames from the envelope's autocorrelation, weighted toward everyday tempos.
fn rough_period(envelope: &[f32], fps: f64) -> Option<f64> {
  let mean = envelope.iter().sum::<f32>() / envelope.len() as f32;
  let centred: Vec<f32> = envelope.iter().map(|v| v - mean).collect();
  let (shortest, longest) = ((60.0 * fps / MAX_BPM).floor() as usize, (60.0 * fps / MIN_BPM).ceil() as usize);
  let correlation = |lag: usize| -> f64 {
    if lag >= centred.len() {
      return 0.0;
    }
    let pairs = centred.len() - lag;
    centred[..pairs].iter().zip(&centred[lag..]).map(|(a, b)| f64::from(a * b)).sum::<f64>() / pairs as f64
  };
  let mut best: Option<(f64, usize)> = None;
  for lag in shortest..=longest.min(centred.len() / 2) {
    // A true beat period also correlates at twice and three times its length.
    let score = correlation(lag) + 0.5 * correlation(lag * 2) + 0.25 * correlation(lag * 3);
    let octaves = (60.0 * fps / lag as f64 / PREFERRED_BPM).log2();
    let weighted = score * (-0.5 * octaves * octaves).exp();
    if best.is_none_or(|(top, _)| weighted > top) {
      best = Some((weighted, lag));
    }
  }
  best.filter(|(score, _)| *score > 0.0).map(|(_, lag)| lag as f64)
}

/// The period (in frames, fractional) and first-beat offset whose grid collects the most onset
/// strength. Autocorrelation is only frame-accurate, which drifts by seconds over a whole song.
fn refine(envelope: &[f32], rough: f64) -> (f64, f64) {
  let at = |frame: f64| -> f64 {
    let i = frame.floor() as usize;
    let frac = (frame - i as f64) as f32;
    match (envelope.get(i), envelope.get(i + 1)) {
      (Some(a), Some(b)) => f64::from(a + (b - a) * frac),
      (Some(a), None) => f64::from(*a),
      _ => 0.0,
    }
  };
  let mut best = (f64::MIN, rough, 0.0);
  for step in -REFINE_STEPS..=REFINE_STEPS {
    let period = rough * (1.0 + REFINE_SPAN * f64::from(step) / f64::from(REFINE_STEPS));
    let beats = (envelope.len() as f64 / period).floor() as usize;
    if beats < 2 {
      continue;
    }
    let mut phase = 0.0;
    while phase < period {
      let score = (0..beats).map(|b| at(phase + b as f64 * period)).sum::<f64>() / beats as f64;
      if score > best.0 {
        best = (score, period, phase);
      }
      phase += 0.5;
    }
  }
  (best.1, best.2)
}

/// Which of the first four beats starts the bars: the one whose every-fourth beats carry the most
/// bass (the kick usually lands on one).
fn downbeat(bass: &[f32], beats: &[f64], fps: f64) -> usize {
  let strength = |time: f64| -> f32 {
    let frame = (time * fps).round() as usize;
    // The onset can sit a frame either side of the grid.
    (frame.saturating_sub(1)..=frame + 1).filter_map(|i| bass.get(i)).copied().fold(0.0, f32::max)
  };
  (0..BEATS_PER_BAR).map(|offset| (offset, beats.iter().skip(offset).step_by(BEATS_PER_BAR).map(|t| strength(*t)).sum::<f32>())).max_by(|a, b| a.1.total_cmp(&b.1)).map_or(0, |(offset, _)| offset)
}

#[cfg(test)]
mod tests {
  use super::*;

  const RATE: u32 = 11_025;

  mod analysis {
    use super::*;

    #[test]
    fn a_click_track_gives_its_tempo_and_beats() {
      let grid = analyse(&click_track(128.0, 24.0, 0.25, None), RATE).unwrap();
      assert!((grid.bpm - 128.0).abs() < 0.5, "bpm {}", grid.bpm);
      let period = 60.0 / 128.0;
      for (i, beat) in grid.beats.iter().enumerate().take(40) {
        let nearest = ((beat - 0.25) / period).round() * period + 0.25;
        assert!((beat - nearest).abs() < 0.03, "beat {i} at {beat:.3}s is off the grid ({nearest:.3}s)");
      }
      assert!((grid.duration - 24.0).abs() < 0.01);
    }

    #[test]
    fn the_grid_doesnt_drift_over_a_whole_song() {
      let grid = analyse(&click_track(140.0, 180.0, 0.1, None), RATE).unwrap();
      let period = 60.0 / 140.0;
      let last = *grid.beats.last().unwrap();
      let nearest = ((last - 0.1) / period).round() * period + 0.1;
      assert!((last - nearest).abs() < 0.04, "last beat {last:.3}s vs {nearest:.3}s");
    }

    #[test]
    fn bars_start_on_the_kick() {
      // Kicks on beats 2, 6, 10… of the click track: the bar starts on the third beat.
      let grid = analyse(&click_track(120.0, 24.0, 0.0, Some(2)), RATE).unwrap();
      let first_kick = 2.0 * 0.5;
      let bar_start = grid.beats[grid.downbeat];
      let off = ((bar_start - first_kick) / 2.0).rem_euclid(1.0);
      assert!(!(0.02..=0.98).contains(&off), "bar starts at {bar_start:.3}s, kicks every 2s from {first_kick}s");
    }

    #[test]
    fn silence_and_short_clips_have_no_grid() {
      assert_eq!(analyse(&vec![0.0; RATE as usize * 20], RATE), None);
      assert_eq!(analyse(&click_track(120.0, 1.0, 0.0, None), RATE), None);
    }
  }

  #[test]
  fn a_steady_grid_covers_the_track() {
    let grid = BeatGrid::steady(120.0, 10.0);
    assert_eq!(grid.beats.len(), 20);
    assert!((grid.period() - 0.5).abs() < 1e-9);
    assert!(grid.beats.iter().all(|t| *t < 10.0));
  }

  /// Short noise bursts every beat from `offset` seconds, with a low thump on every fourth beat
  /// starting at beat `kick_from` when given.
  fn click_track(bpm: f64, seconds: f64, offset: f64, kick_from: Option<usize>) -> Vec<f32> {
    let mut samples = vec![0.0f32; (seconds * f64::from(RATE)) as usize];
    let period = 60.0 / bpm;
    let mut seed = 0x2545_f491u32;
    let mut beat = 0;
    loop {
      let start = ((offset + beat as f64 * period) * f64::from(RATE)) as usize;
      if start >= samples.len() {
        break;
      }
      for i in 0..220.min(samples.len() - start) {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (seed >> 8) as f32 / (1 << 24) as f32 - 0.5;
        samples[start + i] += 0.4 * noise * (1.0 - i as f32 / 220.0);
      }
      if kick_from.is_some_and(|from| beat >= from && (beat - from) % 4 == 0) {
        for i in 0..1400.min(samples.len() - start) {
          let t = i as f32 / RATE as f32;
          samples[start + i] += 0.9 * (2.0 * std::f32::consts::PI * 60.0 * t).sin() * (1.0 - i as f32 / 1400.0);
        }
      }
      beat += 1;
    }
    samples
  }
}
