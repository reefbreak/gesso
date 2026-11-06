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

/// The shape the film is cut to. Veo films in 16:9 or 9:16 only, so the other shapes are cropped
/// from the middle of what was filmed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
  /// The shape the scenes were filmed in.
  #[default]
  AsFilmed,
  /// Instagram Reels and Stories: 9:16.
  Reel,
  /// Instagram's feed portrait: 4:5.
  Feed,
  /// 1:1.
  Square,
}

impl Format {
  pub const ALL: [Format; 4] = [Format::AsFilmed, Format::Reel, Format::Feed, Format::Square];

  pub fn label(self) -> &'static str {
    match self {
      Format::AsFilmed => "As filmed",
      Format::Reel => "Reel 9:16",
      Format::Feed => "Feed 4:5",
      Format::Square => "Square 1:1",
    }
  }

  /// The frame size at `resolution` (`720p` or `1080p`, which for the Instagram shapes is the
  /// width: Instagram shows posts 1080 wide).
  pub fn size(self, aspect: Aspect, resolution: &str) -> (u32, u32) {
    let width = if resolution == "1080p" { 1080 } else { 720 };
    match self {
      Format::AsFilmed => aspect.size(resolution),
      Format::Reel => (width, width * 16 / 9),
      Format::Feed => (width, width * 5 / 4),
      Format::Square => (width, width),
    }
  }

  /// How much of the filmed picture this shape keeps, from 0 to 1 (1 when nothing is cropped).
  pub fn kept(self, aspect: Aspect) -> f64 {
    let ratio = |(w, h): (u32, u32)| f64::from(w) / f64::from(h);
    let (filmed, cut) = (ratio(aspect.size("1080p")), ratio(self.size(aspect, "1080p")));
    (filmed / cut).min(cut / filmed)
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

/// The film's fades from and to black, in seconds (zero for none).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fades {
  pub fade_in: f64,
  pub fade_out: f64,
  /// Whether the sound fades with the picture. Off leaves the song exactly as it is.
  pub audio: bool,
}

impl Fades {
  /// The fades that fit a film of `total` seconds: neither longer than half of it.
  fn within(self, total: f64) -> Self {
    let longest = (total / 2.0).max(0.0);
    Self { fade_in: self.fade_in.clamp(0.0, longest), fade_out: self.fade_out.clamp(0.0, longest), audio: self.audio }
  }

  /// The picture filters for a part running from `start` to `end` of a `total`-second film, with
  /// a leading comma (empty when no fade reaches it). A fade can run across several parts, and
  /// FFmpeg fades on a part's own clock, so the part is moved to the film's clock, faded, and
  /// moved back.
  fn picture(self, start: f64, end: f64, total: f64) -> String {
    let fades = self.within(total);
    let fade_in = (fades.fade_in > 0.0 && start < fades.fade_in).then(|| format!("fade=t=in:st=0:d={:.3}", fades.fade_in));
    let fade_out = (fades.fade_out > 0.0 && end > total - fades.fade_out).then(|| format!("fade=t=out:st={:.3}:d={:.3}", total - fades.fade_out, fades.fade_out));
    let filters: Vec<String> = fade_in.into_iter().chain(fade_out).collect();
    if filters.is_empty() {
      return String::new();
    }
    format!(",setpts=PTS+{start:.3}/TB,{},setpts=PTS-{start:.3}/TB", filters.join(","))
  }

  /// The sound filter for the whole film, when the sound fades too.
  fn sound(self, total: f64) -> Option<String> {
    let fades = self.within(total);
    let fade_in = (fades.fade_in > 0.0).then(|| format!("afade=t=in:st=0:d={:.3}", fades.fade_in));
    let fade_out = (fades.fade_out > 0.0).then(|| format!("afade=t=out:st={:.3}:d={:.3}", total - fades.fade_out, fades.fade_out));
    let filters: Vec<String> = fade_in.into_iter().chain(fade_out).collect();
    (self.audio && !filters.is_empty()).then(|| filters.join(","))
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
pub fn assemble(shots: &[CutShot], audio: &Path, size: (u32, u32), grade: Grade, fades: Fades, out: &Path) -> anyhow::Result<()> {
  anyhow::ensure!(video::ffmpeg_available(), "Cutting the video needs FFmpeg: install it with `brew install ffmpeg`.");
  anyhow::ensure!(!shots.is_empty(), "There are no shots to cut together");
  let dir = tempfile::tempdir()?;
  let (w, h) = (size.0 - size.0 % 2, size.1 - size.1 % 2);
  let fit = format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},setsar=1,fps={FPS}{}", grade.filters());
  let total = shots.last().map_or(0.0, |s| s.end);
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
        format!("{fit},tpad=stop_mode=clone:stop_duration={}{}", shot.end - shot.start + 1.0, fades.picture(shot.start, shot.end, total))
      },
      ShotMedia::Still(path) => {
        args.extend(["-loop".into(), "1".into(), "-framerate".into(), FPS.to_string(), "-i".into(), path.to_string_lossy().into_owned()]);
        format!("{fit}{}", fades.picture(shot.start, shot.end, total))
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
  // The parts share one encoding, so they join without being encoded again; the sound is encoded
  // here, which is where it fades.
  let mut args: Vec<String> = ["-y", "-v", "error", "-f", "concat", "-safe", "0", "-i"].iter().map(|a| (*a).to_owned()).chain([list_path.to_string_lossy().into_owned(), "-i".into(), audio.to_string_lossy().into_owned()]).chain(["-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k"].iter().map(|a| (*a).to_owned())).collect();
  if let Some(sound) = fades.sound(total) {
    args.extend(["-af".into(), sound]);
  }
  args.extend(["-t".into(), format!("{total:.3}"), "-movflags".into(), "+faststart".into(), out.to_string_lossy().into_owned()]);
  run(&args)?;
  anyhow::ensure!(out.exists(), "FFmpeg didn't write the video");
  Ok(())
}

/// A copy of the film sized for uploading from a phone and sharing: H.264 in two passes at a
/// variable bitrate, AAC sound, at most 1080 pixels on its short side, ready to stream. Returns
/// the bitrate aimed for, in kbit/s: [`SHARE_KBPS`], or the film's own when that is lower, since
/// padding a file adds size and no picture.
pub fn share_copy(film: &Path, out: &Path) -> anyhow::Result<u32> {
  anyhow::ensure!(video::ffmpeg_available(), "Exporting the video needs FFmpeg: install it with `brew install ffmpeg`.");
  let seconds = duration(film)?;
  let own_kbps = (std::fs::metadata(film)?.len() as f64 * 8.0 / seconds / 1000.0) as u32;
  let kbps = SHARE_KBPS.min(own_kbps.max(SHARE_KBPS_FLOOR));
  let peak = SHARE_PEAK_KBPS.min(kbps * 5 / 4);
  let dir = tempfile::tempdir()?;
  let log = dir.path().join("pass").to_string_lossy().into_owned();
  let picture: Vec<String> = ["-c:v", "libx264", "-preset", "slow", "-profile:v", "high", "-pix_fmt", "yuv420p", "-vf", SHARE_SCALE].iter().map(|a| (*a).to_owned()).chain(["-b:v".into(), format!("{kbps}k"), "-maxrate".into(), format!("{peak}k"), "-bufsize".into(), format!("{}k", peak * 2), "-passlogfile".into(), log]).collect();
  let source = ["-y".to_owned(), "-v".into(), "error".into(), "-i".into(), film.to_string_lossy().into_owned()];
  // The first pass only measures the film; the second spends the bits where it found they matter.
  run(&[&source[..], &picture[..], &["-pass", "1", "-an", "-f", "null", "-"].map(str::to_owned)].concat())?;
  run(&[&source[..], &picture[..], &["-pass", "2", "-c:a", "aac", "-b:a", SHARE_SOUND, "-ar", "48000", "-movflags", "+faststart"].map(str::to_owned), &[out.to_string_lossy().into_owned()]].concat())?;
  anyhow::ensure!(out.exists(), "FFmpeg didn't write the video");
  Ok(kbps)
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

/// A clip's first `seconds`, silent, re-encoded so it's exactly that long. Lip sync is billed by
/// the second and syncs the whole clip, so only the part the cut will use is sent.
pub fn head_of_clip(clip: &Path, seconds: f64) -> anyhow::Result<Vec<u8>> {
  anyhow::ensure!(video::ffmpeg_available(), "Lip sync needs FFmpeg: install it with `brew install ffmpeg`.");
  let dir = tempfile::tempdir()?;
  let out = dir.path().join("head.mp4");
  let frames = frame_count(0.0, seconds).max(1);
  let filter = format!("fps={FPS},tpad=stop_mode=clone:stop_duration={}", seconds + 1.0);
  let mut args: Vec<String> = ["-y", "-v", "error", "-i"].iter().map(|a| (*a).to_owned()).collect();
  args.extend([clip.to_string_lossy().into_owned(), "-vf".into(), filter, "-frames:v".into(), frames.to_string(), "-an".into()]);
  args.extend(ENCODE.iter().map(|a| (*a).to_owned()));
  args.push(out.to_string_lossy().into_owned());
  run(&args)?;
  Ok(std::fs::read(&out)?)
}

/// The stretch of a track from `start` to `end` seconds, as a WAV.
pub fn slice_of_track(track: &Path, start: f64, end: f64) -> anyhow::Result<Vec<u8>> {
  anyhow::ensure!(video::ffmpeg_available(), "Lip sync needs FFmpeg: install it with `brew install ffmpeg`.");
  anyhow::ensure!(end > start, "The slice ends before it starts");
  let dir = tempfile::tempdir()?;
  let out = dir.path().join("slice.wav");
  run(&["-y", "-v", "error", "-ss", &format!("{start:.3}"), "-i", &track.to_string_lossy(), "-t", &format!("{:.3}", end - start), "-vn", "-ac", "1", "-ar", "44100", &out.to_string_lossy()].map(str::to_owned))?;
  Ok(std::fs::read(&out)?)
}

/// One encoding for every part, so the concat can copy.
/// The share copy's picture: the bitrate aimed for and the most it may peak at, in kbit/s.
pub const SHARE_KBPS: u32 = 12_000;
pub const SHARE_PEAK_KBPS: u32 = 15_000;
/// The least the share copy aims for, however small the film's own file is.
const SHARE_KBPS_FLOOR: u32 = 2_000;
const SHARE_SOUND: &str = "256k";
/// At most 1080 pixels on the short side, never enlarged, both sides even.
const SHARE_SCALE: &str = "scale='if(gt(iw,ih),-2,min(1080,iw))':'if(gt(iw,ih),min(1080,ih),-2)'";

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
    let shots_clip = long.clone();
    let shots_still = make(&["color=c=white:size=400x400", "-frames:v", "1"], "white.png");
    let shots = [CutShot { start: 0.0, end: 1.7, media: ShotMedia::Clip(long) }, CutShot { start: 1.7, end: 3.9, media: ShotMedia::Clip(short) }, CutShot { start: 3.9, end: 5.5, media: ShotMedia::Still(still) }];
    let out = dir.path().join("cut.mp4");
    assemble(&shots, &song, Aspect::Wide.size("720p"), Grade::Mono, Fades::default(), &out).unwrap();

    let (w, h, fps, seconds) = video::probe_file(&out).unwrap();
    assert_eq!((w, h), (1280, 720));
    assert!((fps - 24.0).abs() < 0.01, "fps {fps}");
    assert!((seconds - 5.5).abs() < 0.1, "the cut runs {seconds:.2}s");
    assert!((duration(&song).unwrap() - 6.0).abs() < 0.05);
    let (copy, mime) = listening_copy(&song).unwrap();
    assert!(copy.len() > 1000 && mime == "audio/aac");

    // Lip sync gets only what the cut uses: the clip's first 1.7 s and the song under it.
    let head = dir.path().join("head.mp4");
    std::fs::write(&head, head_of_clip(&shots_clip, 1.7).unwrap()).unwrap();
    let (_, _, _, head_seconds) = video::probe_file(&head).unwrap();
    assert!((head_seconds - 1.7).abs() < 0.06, "the head runs {head_seconds:.2}s");
    let slice = dir.path().join("slice.wav");
    std::fs::write(&slice, slice_of_track(&song, 1.7, 3.9).unwrap()).unwrap();
    assert!((duration(&slice).unwrap() - 2.2).abs() < 0.05);
    assert!(slice_of_track(&song, 3.0, 3.0).is_err());

    // Faded: black at both ends, the picture in between, and a fade that began in the first
    // shot carried on through the cut into the second.
    let faded = dir.path().join("faded.mp4");
    let bright = [CutShot { start: 0.0, end: 1.0, media: ShotMedia::Still(shots_still.clone()) }, CutShot { start: 1.0, end: 4.0, media: ShotMedia::Still(shots_still) }];
    assemble(&bright, &song, (320, 180), Grade::None, Fades { fade_in: 2.0, fade_out: 1.0, audio: true }, &faded).unwrap();
    let level = |at: f64| -> u32 {
      let frame = video::command("ffmpeg").args(["-v", "error", "-ss", &format!("{at:.3}"), "-i"]).arg(&faded).args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "-"]).output().unwrap().stdout;
      frame.iter().map(|p| u32::from(*p)).sum::<u32>() / frame.len().max(1) as u32
    };
    let (first, before_cut, after_cut, middle, last) = (level(0.0), level(0.9), level(1.1), level(2.5), level(3.95));
    assert!(first < 12, "the film opens on black ({first})");
    assert!(before_cut > first + 10 && after_cut > before_cut && after_cut < middle, "the fade carries on across the cut: {before_cut}, {after_cut}, then {middle}");
    assert!(last < middle / 4, "and ends on black ({last} against {middle})");
    let (_, _, _, faded_seconds) = video::probe_file(&faded).unwrap();
    assert!((faded_seconds - 4.0).abs() < 0.1, "fading doesn't change the length ({faded_seconds:.2}s)");

    // An Instagram feed post cut from a wide film: 4:5, the same length, the middle of the frame.
    let feed = dir.path().join("feed.mp4");
    assemble(&shots, &song, Format::Feed.size(Aspect::Wide, "720p"), Grade::None, Fades::default(), &feed).unwrap();
    let (w, h, _, feed_seconds) = video::probe_file(&feed).unwrap();
    assert_eq!((w, h), (720, 900));
    assert!((feed_seconds - 5.5).abs() < 0.1, "the shape doesn't change the length ({feed_seconds:.2}s)");
  }

  /// The share copy is at most 1080 across, keeps its sound, and is no bigger than the bitrate
  /// allows: a busy film is brought down to the target, a small one isn't padded up to it.
  #[test]
  fn the_share_copy_is_sized_for_upload() {
    if !video::ffmpeg_available() {
      return;
    }
    let dir = tempfile::tempdir().unwrap();
    let film = |source: &str, quality: &str, name: &str| -> PathBuf {
      let path = dir.path().join(name);
      run(&["-y", "-v", "error", "-f", "lavfi", "-i", source, "-f", "lavfi", "-i", "sine=frequency=440:duration=3", "-c:v", "libx264", "-preset", "ultrafast", "-crf", quality, "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest", &path.to_string_lossy()].map(str::to_owned)).unwrap();
      path
    };
    let kbps_of = |path: &Path| std::fs::metadata(path).unwrap().len() as f64 * 8.0 / duration(path).unwrap() / 1000.0;
    // Noise is the hardest thing to encode: far over the target at this size.
    let busy = film("nullsrc=size=1440x2560:rate=12:duration=3,geq=random(1)*255:128:128", "10", "busy.mp4");
    let shared = dir.path().join("busy_instagram.mp4");
    assert_eq!(share_copy(&busy, &shared).unwrap(), SHARE_KBPS);
    let (w, h, _, seconds) = video::probe_file(&shared).unwrap();
    assert_eq!((w, h), (1080, 1920));
    assert!((seconds - 3.0).abs() < 0.2);
    assert!(kbps_of(&busy) > f64::from(SHARE_PEAK_KBPS) && kbps_of(&shared) < f64::from(SHARE_PEAK_KBPS) + 300.0, "{} -> {}", kbps_of(&busy), kbps_of(&shared));
    let sound = video::command("ffprobe").args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=codec_name", "-of", "csv=p=0"]).arg(&shared).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&sound.stdout).trim(), "aac");
    // A film already smaller than the target keeps its size and its shape.
    let calm = film("color=c=gray:size=640x360:rate=24:duration=3", "23", "calm.mp4");
    let shared = dir.path().join("calm_instagram.mp4");
    assert!(share_copy(&calm, &shared).unwrap() < SHARE_KBPS);
    let (w, h, _, _) = video::probe_file(&shared).unwrap();
    assert_eq!((w, h), (640, 360));
  }

  #[test]
  fn instagram_shapes_have_their_sizes_and_say_what_they_crop() {
    assert_eq!(Format::AsFilmed.size(Aspect::Wide, "1080p"), (1920, 1080));
    assert_eq!(Format::AsFilmed.size(Aspect::Tall, "720p"), (720, 1280));
    assert_eq!((Format::Reel.size(Aspect::Tall, "1080p"), Format::Feed.size(Aspect::Tall, "1080p"), Format::Square.size(Aspect::Wide, "1080p")), ((1080, 1920), (1080, 1350), (1080, 1080)));
    assert_eq!((Format::Reel.size(Aspect::Wide, "720p"), Format::Feed.size(Aspect::Wide, "720p")), ((720, 1280), (720, 900)));
    // A reel cut from a tall film keeps everything; a feed post keeps most; from a wide film, little.
    assert!((Format::Reel.kept(Aspect::Tall) - 1.0).abs() < 1e-9 && (Format::AsFilmed.kept(Aspect::Wide) - 1.0).abs() < 1e-9);
    assert!((Format::Feed.kept(Aspect::Tall) - 0.703).abs() < 0.001, "{}", Format::Feed.kept(Aspect::Tall));
    assert!((Format::Reel.kept(Aspect::Wide) - 0.316).abs() < 0.001, "{}", Format::Reel.kept(Aspect::Wide));
    assert!((Format::Square.kept(Aspect::Wide) - 0.5625).abs() < 0.001);
  }

  #[test]
  fn fades_reach_only_the_parts_they_cover() {
    let fades = Fades { fade_in: 2.0, fade_out: 1.0, audio: false };
    assert_eq!(fades.picture(0.0, 1.5, 60.0), ",setpts=PTS+0.000/TB,fade=t=in:st=0:d=2.000,setpts=PTS-0.000/TB");
    assert_eq!(fades.picture(1.5, 4.0, 60.0), ",setpts=PTS+1.500/TB,fade=t=in:st=0:d=2.000,setpts=PTS-1.500/TB", "the second shot starts inside the fade");
    assert_eq!(fades.picture(4.0, 8.0, 60.0), "");
    assert_eq!(fades.picture(57.0, 60.0, 60.0), ",setpts=PTS+57.000/TB,fade=t=out:st=59.000:d=1.000,setpts=PTS-57.000/TB");
    assert_eq!(Fades::default().picture(0.0, 2.0, 60.0), "");
    assert_eq!(fades.sound(60.0), None, "the song is left alone unless asked");
    assert_eq!(Fades { audio: true, ..fades }.sound(60.0).as_deref(), Some("afade=t=in:st=0:d=2.000,afade=t=out:st=59.000:d=1.000"));
    // A film shorter than its fades: each takes half at most.
    assert_eq!(Fades { fade_in: 5.0, fade_out: 5.0, audio: true }.sound(4.0).as_deref(), Some("afade=t=in:st=0:d=2.000,afade=t=out:st=2.000:d=2.000"));
  }
}
