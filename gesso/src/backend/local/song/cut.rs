//! The final cut, on the user's machine with FFmpeg: every shot trimmed to its slot (to the exact
//! frame, so the picture can't drift off the beat over a whole song), graded, joined, and laid
//! under the song.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::backend::video;

/// The cut's frame rate (Veo's own).
pub const FPS: f64 = 24.0;
/// Bitrate of the small copy of the song a listening model is sent.
const LISTEN_BITRATE: &str = "64k";

/// The frame's shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aspect {
  #[default]
  Wide,
  Tall,
}

impl Aspect {
  /// As the providers write it.
  pub fn ratio(self) -> &'static str {
    match self {
      Aspect::Wide => "16:9",
      Aspect::Tall => "9:16",
    }
  }

  /// The cut's frame size at `resolution` (`720p` or `1080p`).
  pub fn size(self, resolution: &str) -> (u32, u32) {
    let (long, short) = if resolution == "1080p" { (1920, 1080) } else { (1280, 720) };
    match self {
      Aspect::Wide => (long, short),
      Aspect::Tall => (short, long),
    }
  }
}

/// A finish applied to every shot, so clips from different generations sit together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
  /// The clips as generated.
  #[default]
  None,
  /// Black and white, crushed contrast, heavy grain.
  Mono,
  /// Colour, with contrast and grain.
  Gritty,
}

impl Grade {
  pub const ALL: [Grade; 3] = [Grade::None, Grade::Mono, Grade::Gritty];

  pub fn label(self) -> &'static str {
    match self {
      Grade::None => "As shot",
      Grade::Mono => "Black & white",
      Grade::Gritty => "Gritty colour",
    }
  }

  /// FFmpeg filters for the finish, with a leading comma (empty for none).
  fn filters(self) -> &'static str {
    match self {
      Grade::None => "",
      Grade::Mono => ",hue=s=0,eq=contrast=1.3:brightness=-0.03,noise=alls=16:allf=t",
      Grade::Gritty => ",eq=contrast=1.15:saturation=0.85,noise=alls=10:allf=t",
    }
  }
}

/// What fills a slot in the cut.
#[derive(Clone, Debug, PartialEq)]
pub enum ShotMedia {
  Clip(PathBuf),
  /// Held for the slot's length, for a shot whose clip hasn't been made.
  Still(PathBuf),
}

/// One slot of the cut.
#[derive(Clone, Debug, PartialEq)]
pub struct CutShot {
  pub start: f64,
  pub end: f64,
  pub media: ShotMedia,
}

/// Joins `shots` in order under `audio`, writing an MP4 to `out`. Blocking.
pub fn assemble(shots: &[CutShot], audio: &Path, size: (u32, u32), grade: Grade, out: &Path) -> anyhow::Result<()> {
  anyhow::ensure!(video::ffmpeg_available(), "Cutting the video needs FFmpeg: install it with `brew install ffmpeg`.");
  anyhow::ensure!(!shots.is_empty(), "There are no shots to cut together");
  let dir = tempfile::tempdir()?;
  let (w, h) = (size.0 - size.0 % 2, size.1 - size.1 % 2);
  let fit = format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},setsar=1,fps={FPS}{}", grade.filters());
  let mut list = String::new();
  for (i, shot) in shots.iter().enumerate() {
    let frames = frame_count(shot.start, shot.end);
    if frames == 0 {
      continue;
    }
    let part = dir.path().join(format!("part_{i:04}.mp4"));
    let mut args: Vec<String> = vec!["-y".into(), "-v".into(), "error".into()];
    let filter = match &shot.media {
      ShotMedia::Clip(path) => {
        args.extend(["-i".into(), path.to_string_lossy().into_owned()]);
        // A clip shorter than its slot holds its last frame.
        format!("{fit},tpad=stop_mode=clone:stop_duration={}", shot.end - shot.start + 1.0)
      },
      ShotMedia::Still(path) => {
        args.extend(["-loop".into(), "1".into(), "-framerate".into(), FPS.to_string(), "-i".into(), path.to_string_lossy().into_owned()]);
        fit.clone()
      },
    };
    args.extend(["-vf".into(), filter, "-frames:v".into(), frames.to_string(), "-an".into()]);
    args.extend(ENCODE.iter().map(|a| (*a).to_owned()));
    args.push(part.to_string_lossy().into_owned());
    run(&args).map_err(|e| e.context(format!("Shot {}", i + 1)))?;
    list.push_str(&format!("file '{}'\n", part.to_string_lossy().replace('\'', "'\\''")));
  }
  anyhow::ensure!(!list.is_empty(), "Every shot is shorter than a frame");
  let list_path = dir.path().join("parts.txt");
  std::fs::write(&list_path, list)?;
  let seconds = shots.last().map_or(0.0, |s| s.end);
  // The parts share one encoding, so they join without being encoded again.
  let args: Vec<String> = ["-y", "-v", "error", "-f", "concat", "-safe", "0", "-i"].iter().map(|a| (*a).to_owned()).chain([list_path.to_string_lossy().into_owned(), "-i".into(), audio.to_string_lossy().into_owned()]).chain(["-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-t"].iter().map(|a| (*a).to_owned())).chain([format!("{seconds:.3}"), "-movflags".into(), "+faststart".into(), out.to_string_lossy().into_owned()]).collect();
  run(&args)?;
  anyhow::ensure!(out.exists(), "FFmpeg didn't write the video");
  Ok(())
}

/// A track's length in seconds.
pub fn duration(path: &Path) -> anyhow::Result<f64> {
  anyhow::ensure!(video::ffmpeg_available(), "Reading the track needs FFmpeg: install it with `brew install ffmpeg`.");
  let output = video::command("ffprobe").args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"]).arg(path).output()?;
  String::from_utf8_lossy(&output.stdout).trim().parse::<f64>().ok().filter(|d| *d > 0.0).ok_or_else(|| anyhow::anyhow!("Couldn't read the length of {}", path.display()))
}

/// A small mono AAC copy of the track, for a model to listen to (a WAV of a whole song is too big
/// to send inline). Returns the bytes and their MIME type.
pub fn listening_copy(path: &Path) -> anyhow::Result<(Vec<u8>, &'static str)> {
  anyhow::ensure!(video::ffmpeg_available(), "Reading the track needs FFmpeg: install it with `brew install ffmpeg`.");
  let dir = tempfile::tempdir()?;
  let out = dir.path().join("listen.aac");
  run(&["-y", "-v", "error", "-i", &path.to_string_lossy(), "-vn", "-ac", "1", "-c:a", "aac", "-b:a", LISTEN_BITRATE, "-f", "adts", &out.to_string_lossy()].map(str::to_owned))?;
  Ok((std::fs::read(&out)?, "audio/aac"))
}

/// One encoding for every part, so the concat can copy.
const ENCODE: [&str; 10] = ["-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p", "-video_track_timescale", "12288"];

/// Whole frames in `[start, end)`, counted from the song's start so rounding never accumulates.
fn frame_count(start: f64, end: f64) -> u64 {
  let frame = |t: f64| (t.max(0.0) * FPS).round() as u64;
  frame(end).saturating_sub(frame(start))
}

fn run(args: &[String]) -> anyhow::Result<()> {
  let result = video::command("ffmpeg").args(args).stdout(Stdio::null()).stderr(Stdio::piped()).output()?;
  anyhow::ensure!(result.status.success(), "FFmpeg failed: {}", String::from_utf8_lossy(&result.stderr).lines().last().unwrap_or("unknown error"));
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn frames_are_counted_from_the_songs_start() {
    // 0.43 s shots: each rounds to 10 or 11 frames, and 100 of them come to exactly 43 s.
    let total: u64 = (0..100).map(|i| frame_count(f64::from(i) * 0.43, f64::from(i + 1) * 0.43)).sum();
    assert_eq!(total, (43.0 * FPS) as u64);
    assert_eq!(frame_count(1.0, 1.0), 0);
  }

  #[test]
  fn frame_sizes_follow_the_aspect() {
    assert_eq!(Aspect::Wide.size("720p"), (1280, 720));
    assert_eq!(Aspect::Tall.size("1080p"), (1080, 1920));
    assert_eq!((Aspect::Wide.ratio(), Aspect::Tall.ratio()), ("16:9", "9:16"));
  }

  /// The real thing, when FFmpeg is installed: two clips and a still under a tone, cut to slots.
  #[test]
  fn shots_are_cut_to_their_slots_under_the_song() {
    if !video::ffmpeg_available() {
      return;
    }
    let dir = tempfile::tempdir().unwrap();
    let make = |args: &[&str], name: &str| -> PathBuf {
      let path = dir.path().join(name);
      let mut all: Vec<String> = ["-y", "-v", "error", "-f", "lavfi", "-i"].iter().map(|a| (*a).to_owned()).collect();
      all.extend(args.iter().map(|a| (*a).to_owned()));
      all.push(path.to_string_lossy().into_owned());
      run(&all).unwrap();
      path
    };
    let long = make(&["testsrc=size=640x360:rate=24:duration=4", "-pix_fmt", "yuv420p"], "long.mp4");
    // Shorter than its slot: the last frame is held.
    let short = make(&["testsrc2=size=320x240:rate=30:duration=1", "-pix_fmt", "yuv420p"], "short.mp4");
    let still = make(&["color=c=red:size=400x400", "-frames:v", "1"], "still.png");
    let song = make(&["sine=frequency=440:duration=6"], "song.wav");
    let shots = [CutShot { start: 0.0, end: 1.7, media: ShotMedia::Clip(long) }, CutShot { start: 1.7, end: 3.9, media: ShotMedia::Clip(short) }, CutShot { start: 3.9, end: 5.5, media: ShotMedia::Still(still) }];
    let out = dir.path().join("cut.mp4");
    assemble(&shots, &song, Aspect::Wide.size("720p"), Grade::Mono, &out).unwrap();

    let (w, h, fps, seconds) = video::probe_file(&out).unwrap();
    assert_eq!((w, h), (1280, 720));
    assert!((fps - 24.0).abs() < 0.01, "fps {fps}");
    assert!((seconds - 5.5).abs() < 0.1, "the cut runs {seconds:.2}s");
    assert!((duration(&song).unwrap() - 6.0).abs() < 0.05);
    let (copy, mime) = listening_copy(&song).unwrap();
    assert!(copy.len() > 1000 && mime == "audio/aac");
  }
}
