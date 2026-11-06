//! The tool pages' work: frame extraction, background removal, image edits and video edits. Each
//! tool takes files from disk (the library or the user's own), saves its results into the local
//! library as one batch, records the charge, and leaves a record for the lightbox.
//!
//! Frame extraction, trimming, joining and local background removal run on the user's machine
//! (FFmpeg and ISNet) and cost nothing. Image edits run on Nano Banana (Vertex) or Stability's
//! image services (Bedrock); extending a clip runs on Veo (Vertex), and describing an edit to a
//! clip ("remove the dog's tail") runs on Gemini Omni Flash (Vertex), 10 seconds at a time.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

use super::GeneratedFile;
use super::RefImage;
use super::pricing::PriceBook;
use super::song::cut::{self, CutShot, Fades, Grade, ShotMedia};
use super::spend::Account;
use super::vertex::{GeminiImageRequest, OMNI_MODELS, VeoRequest};
use crate::backend::video;

/// How long a Veo extension adds.
pub const EXTEND_SECONDS: f64 = 7.0;
/// The longest clip Omni Flash edits in one call; longer clips are edited in parts.
pub const OMNI_MAX_SECONDS: f64 = 10.0;
/// The longest clip a Describe edit takes (six parts), so one click can't run up a large bill.
pub const OMNI_LONGEST_CLIP: f64 = 60.0;
/// Omni Flash's shortest output; a shorter final part is merged into the one before it.
const OMNI_MIN_SECONDS: f64 = 3.0;
/// Omni Flash's output resolutions offered on the page, the default (720p, native) first.
pub const OMNI_RESOLUTIONS: [&str; 3] = ["720p", "1080p", "360p"];
/// Added to every Describe edit, as Google recommends, so only the asked-for change is made.
const KEEP_THE_REST: &str = "Keep everything else the same.";
/// What Gemini is told when a mask comes with an edit.
const MASK_INSTRUCTION: &str = "The second image is a mask. Change only the areas that are white in the mask, blending the edit naturally into its surroundings, and keep everything in the black areas exactly as it is.";

/// A tool run, with the files it works on.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolRequest {
  /// Stills at these times (seconds).
  ExtractFrames {
    video: PathBuf,
    times: Vec<f64>,
  },
  RemoveBackground {
    image: PathBuf,
    engine: BackgroundEngine,
  },
  /// `mask` is a PNG the size of the image, white where the edit applies.
  EditImage {
    image: PathBuf,
    mask: Option<Vec<u8>>,
    op: EditOp,
  },
  TrimVideo {
    video: PathBuf,
    start: f64,
    end: f64,
  },
  /// Clips in order; later ones are scaled to the first one's frame size.
  JoinVideos {
    videos: Vec<PathBuf>,
  },
  /// A film cut from its scenes, each trimmed to its length, over one soundtrack.
  CutFilm {
    film: Film,
  },
  ExtendVideo {
    video: PathBuf,
    prompt: String,
    tier: VeoTier,
  },
  /// Edit a clip by describing the change (Gemini Omni Flash). `seconds` is the clip's length,
  /// for the estimate.
  DescribeEdit {
    video: PathBuf,
    prompt: String,
    seconds: f64,
    /// One of [`OMNI_RESOLUTIONS`].
    resolution: String,
    /// Put the original clip's sound back (Omni Flash makes its own).
    keep_sound: bool,
  },
  /// Keep an image (a 3D Stage render) in the library as is.
  SaveImage {
    image: PathBuf,
  },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BackgroundEngine {
  /// ISNet on this machine: free.
  #[default]
  Local,
  /// Stability's Remove Background on Bedrock.
  Bedrock,
}

/// What an image edit does.
#[derive(Clone, Debug, PartialEq)]
pub enum EditOp {
  /// Describe the change; Nano Banana makes it (inside the mask, when there is one).
  Instruct { pro: bool, prompt: String, image_size: Option<String> },
  /// Repaint the masked area from a prompt (Stability Inpaint).
  Inpaint { prompt: String },
  /// Remove what's under the mask (Stability Erase Object).
  Erase,
  /// Find something by description and replace it (Stability Search and Replace).
  SearchReplace { search: String, prompt: String },
  /// Extend the canvas by these pixels (Stability Outpaint).
  Outpaint { left: u32, right: u32, up: u32, down: u32, prompt: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VeoTier {
  #[default]
  Fast,
  Standard,
  Lite,
}

impl VeoTier {
  pub fn model(self) -> &'static str {
    match self {
      VeoTier::Fast => "veo-3.1-fast-generate-001",
      VeoTier::Standard => "veo-3.1-generate-001",
      VeoTier::Lite => "veo-3.1-lite-generate-001",
    }
  }

  pub fn label(self) -> &'static str {
    match self {
      VeoTier::Fast => "Veo 3.1 Fast",
      VeoTier::Standard => "Veo 3.1",
      VeoTier::Lite => "Veo 3.1 Lite",
    }
  }
}

/// Where a tool runs and what it's called, for records and the spend ledger.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolRoute {
  /// The record's model id (`tool_inpaint`).
  pub id: &'static str,
  pub name: &'static str,
  pub provider: &'static str,
  /// The API model, or the local program.
  pub upstream: &'static str,
  pub account: Option<Account>,
}

impl ToolRequest {
  pub fn route(&self) -> ToolRoute {
    let local = |id, name, upstream| ToolRoute { id, name, provider: "Your Mac", upstream, account: None };
    let bedrock = |id, name, upstream| ToolRoute { id, name, provider: "Bedrock", upstream, account: Some(Account::Bedrock) };
    match self {
      ToolRequest::ExtractFrames { .. } => local("tool_extract_frames", "Frame Extract", "ffmpeg"),
      ToolRequest::RemoveBackground { engine: BackgroundEngine::Local, .. } => local("tool_remove_background", "Remove Background (local)", "isnet-general-use"),
      ToolRequest::RemoveBackground { engine: BackgroundEngine::Bedrock, .. } => bedrock("tool_remove_background", "Remove Background", "stability.stable-image-remove-background-v1:0"),
      ToolRequest::EditImage { op: EditOp::Instruct { pro: true, .. }, .. } => ToolRoute { id: "tool_edit_image", name: "Edit with Nano Banana Pro", provider: "Vertex AI", upstream: "gemini-3-pro-image", account: Some(Account::Vertex) },
      ToolRequest::EditImage { op: EditOp::Instruct { pro: false, .. }, .. } => ToolRoute { id: "tool_edit_image", name: "Edit with Nano Banana 2", provider: "Vertex AI", upstream: "gemini-3.1-flash-image", account: Some(Account::Vertex) },
      ToolRequest::EditImage { op: EditOp::Inpaint { .. }, .. } => bedrock("tool_inpaint", "Inpaint", "stability.stable-image-inpaint-v1:0"),
      ToolRequest::EditImage { op: EditOp::Erase, .. } => bedrock("tool_erase", "Erase Object", "stability.stable-image-erase-object-v1:0"),
      ToolRequest::EditImage { op: EditOp::SearchReplace { .. }, .. } => bedrock("tool_search_replace", "Search and Replace", "stability.stable-image-search-replace-v1:0"),
      ToolRequest::EditImage { op: EditOp::Outpaint { .. }, .. } => bedrock("tool_outpaint", "Outpaint", "stability.stable-outpaint-v1:0"),
      ToolRequest::TrimVideo { .. } => local("tool_trim_video", "Trim", "ffmpeg"),
      ToolRequest::SaveImage { .. } => local("tool_stage_render", "3D Stage render", "stage"),
      ToolRequest::JoinVideos { .. } => local("tool_join_videos", "Join", "ffmpeg"),
      ToolRequest::CutFilm { .. } => local("tool_cut_film", "Film", "ffmpeg"),
      ToolRequest::DescribeEdit { .. } => ToolRoute { id: "tool_describe_edit", name: "Edit with Gemini Omni Flash", provider: "Vertex AI", upstream: OMNI_MODELS[0], account: Some(Account::Vertex) },
      ToolRequest::ExtendVideo { tier, .. } => ToolRoute {
        id: "tool_extend_video",
        name: match tier {
          VeoTier::Fast => "Extend with Veo 3.1 Fast",
          VeoTier::Standard => "Extend with Veo 3.1",
          VeoTier::Lite => "Extend with Veo 3.1 Lite",
        },
        provider: "Vertex AI",
        upstream: tier.model(),
        account: Some(Account::Vertex),
      },
    }
  }

  /// What the run will cost at list prices (zero for the local tools).
  pub fn estimate(&self, prices: &PriceBook) -> f64 {
    let route = self.route();
    match self {
      ToolRequest::EditImage { op: EditOp::Instruct { image_size, .. }, mask, .. } => {
        let size = image_size.as_deref().and_then(super::catalog::image_size);
        let output = prices.gemini_call(route.upstream, None, size).unwrap_or(0.0);
        let inputs = 1 + usize::from(mask.is_some());
        let per_input = if route.upstream == "gemini-3-pro-image" { 560.0 } else { 1120.0 };
        output + prices.gemini_rates(route.upstream).map_or(0.0, |r| inputs as f64 * per_input * r.input / 1_000_000.0)
      },
      ToolRequest::EditImage { .. } | ToolRequest::RemoveBackground { engine: BackgroundEngine::Bedrock, .. } => prices.stability_service(route.upstream).unwrap_or(0.0),
      ToolRequest::ExtendVideo { tier, .. } => prices.veo_per_second(tier.model(), true, "720p").unwrap_or(0.0) * EXTEND_SECONDS,
      ToolRequest::DescribeEdit { seconds, resolution, .. } => prices.omni_edit_estimate(*seconds, resolution),
      _ => 0.0,
    }
  }
}

/// Stills from a clip, as PNGs, in time order.
pub fn extract_frames(video: &Path, times: &[f64]) -> anyhow::Result<Vec<GeneratedFile>> {
  anyhow::ensure!(video::ffmpeg_available(), "Frame Extract needs FFmpeg: install it with `brew install ffmpeg`.");
  let dir = tempfile::tempdir()?;
  let mut times: Vec<f64> = times.iter().copied().filter(|t| t.is_finite() && *t >= 0.0).collect();
  times.sort_by(f64::total_cmp);
  times.dedup_by(|a, b| (*a - *b).abs() < 0.01);
  anyhow::ensure!(!times.is_empty(), "Pick at least one moment to extract");
  let mut out = Vec::new();
  for (i, t) in times.iter().enumerate() {
    let frame = dir.path().join(format!("frame_{i}.png"));
    run_ffmpeg(&["-y", "-loglevel", "error", "-ss", &format!("{t:.3}"), "-i"], video, &["-frames:v", "1"], &frame)?;
    out.push(GeneratedFile { bytes: std::fs::read(&frame)?, extension: "png", usage: None, via_key: false });
  }
  Ok(out)
}

/// A clip cut to `[start, end)` seconds, re-encoded so the cut lands on the exact frame.
pub fn trim_video(video: &Path, start: f64, end: f64) -> anyhow::Result<GeneratedFile> {
  anyhow::ensure!(end > start + 0.05, "The end must come after the start");
  let dir = tempfile::tempdir()?;
  let out = dir.path().join("trimmed.mp4");
  run_ffmpeg(&["-y", "-loglevel", "error", "-ss", &format!("{start:.3}"), "-i"], video, &["-t", &format!("{:.3}", end - start), "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p", "-c:a", "aac", "-movflags", "+faststart"], &out)?;
  Ok(GeneratedFile { bytes: std::fs::read(&out)?, extension: "mp4", usage: None, via_key: false })
}

/// A film as its parts, so it can be edited as one: the scenes in order, each a clip or a still
/// held for its length, and the soundtrack that runs under all of them.
#[derive(Clone, Debug, PartialEq)]
pub struct Film {
  pub title: String,
  pub scenes: Vec<FilmScene>,
  pub audio: Option<PathBuf>,
  /// The frame size it is cut at.
  pub size: (u32, u32),
  pub grade: Grade,
  pub fades: Fades,
}

/// One scene of a film.
#[derive(Clone, Debug, PartialEq)]
pub struct FilmScene {
  /// What the scene is called in the list ("12 \u{b7} Verse 1").
  pub label: String,
  pub media: PathBuf,
  /// A picture held for the scene's length, where there is no clip.
  pub still: bool,
  /// How long the scene runs in the film; a longer clip is cut off, a shorter one holds its last frame.
  pub seconds: f64,
  /// What is sung over it, for the list.
  pub lyric: String,
}

impl Film {
  pub fn seconds(&self) -> f64 {
    self.scenes.iter().map(|scene| scene.seconds).sum()
  }

  /// The scenes as the cut takes them: end to end from zero, in the order they are in now.
  fn shots(&self) -> Vec<CutShot> {
    let mut at = 0.0;
    self
      .scenes
      .iter()
      .map(|scene| {
        let start = at;
        at += scene.seconds;
        CutShot { start, end: at, media: if scene.still { ShotMedia::Still(scene.media.clone()) } else { ShotMedia::Clip(scene.media.clone()) } }
      })
      .collect()
  }
}

/// Cuts a film from its scenes with the frame-exact cut the Music Video page uses, so scenes keep
/// their lengths and the soundtrack stays in time.
pub fn cut_film(film: &Film) -> anyhow::Result<GeneratedFile> {
  anyhow::ensure!(!film.scenes.is_empty(), "The film has no scenes");
  let audio = film.audio.as_ref().ok_or_else(|| anyhow::anyhow!("The film has no soundtrack. Pick one first."))?;
  let dir = tempfile::tempdir()?;
  let out = dir.path().join("film.mp4");
  cut::assemble(&film.shots(), audio, film.size, film.grade, film.fades, &out)?;
  Ok(GeneratedFile { bytes: std::fs::read(&out)?, extension: "mp4", usage: None, via_key: false })
}

/// Clips joined end to end at the first clip's frame size (others letterboxed), 24 fps. Sound is
/// kept only when every clip has it.
pub fn join_videos(videos: &[PathBuf]) -> anyhow::Result<GeneratedFile> {
  anyhow::ensure!(videos.len() >= 2, "Add at least two clips to join");
  anyhow::ensure!(video::ffmpeg_available(), "Joining clips needs FFmpeg: install it with `brew install ffmpeg`.");
  let (w, h, _, _) = video::probe_file(&videos[0]).map_err(|e| anyhow::anyhow!("Couldn't read {}: {e}", videos[0].display()))?;
  let (w, h) = (w - w % 2, h - h % 2);
  let audio = videos.iter().all(|v| has_audio(v));
  let mut args: Vec<String> = vec!["-y".into(), "-loglevel".into(), "error".into()];
  for v in videos {
    args.push("-i".into());
    args.push(v.to_string_lossy().into_owned());
  }
  let mut filter = String::new();
  for i in 0..videos.len() {
    filter.push_str(&format!("[{i}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=24[v{i}];"));
  }
  for i in 0..videos.len() {
    filter.push_str(&format!("[v{i}]"));
    if audio {
      filter.push_str(&format!("[{i}:a]"));
    }
  }
  filter.push_str(&format!("concat=n={}:v=1:a={}[v]{}", videos.len(), u8::from(audio), if audio { "[a]" } else { "" }));
  let dir = tempfile::tempdir()?;
  let out = dir.path().join("joined.mp4");
  args.extend(["-filter_complex".into(), filter, "-map".into(), "[v]".into()]);
  if audio {
    args.extend(["-map".into(), "[a]".into(), "-c:a".into(), "aac".into()]);
  }
  args.extend(["-c:v".into(), "libx264".into(), "-preset".into(), "veryfast".into(), "-crf".into(), "18".into(), "-pix_fmt".into(), "yuv420p".into(), "-movflags".into(), "+faststart".into()]);
  args.push(out.to_string_lossy().into_owned());
  let status = video::command("ffmpeg").args(&args).stdout(Stdio::null()).stderr(Stdio::piped()).output()?;
  anyhow::ensure!(status.status.success(), "FFmpeg couldn't join the clips: {}", String::from_utf8_lossy(&status.stderr).lines().last().unwrap_or("unknown error"));
  Ok(GeneratedFile { bytes: std::fs::read(&out)?, extension: "mp4", usage: None, via_key: false })
}

/// A clip made ready for Veo extension: MP4, 24 fps, 720p (landscape or portrait), at most 30 s.
pub fn prepare_for_extension(video: &Path) -> anyhow::Result<Vec<u8>> {
  let (w, h, _, duration) = video::probe_file(video).map_err(|e| anyhow::anyhow!("Couldn't read the clip: {e}"))?;
  anyhow::ensure!(duration <= 30.5, "Veo can extend clips up to 30 seconds; trim this one first");
  let (tw, th) = if w >= h { (1280, 720) } else { (720, 1280) };
  let dir = tempfile::tempdir()?;
  let out = dir.path().join("for_veo.mp4");
  let scale = format!("scale={tw}:{th}:force_original_aspect_ratio=decrease,pad={tw}:{th}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=24");
  run_ffmpeg(&["-y", "-loglevel", "error", "-i"], video, &["-vf", &scale, "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p", "-c:a", "aac"], &out)?;
  Ok(std::fs::read(&out)?)
}

/// How a clip is cut for Omni Flash: `(start, length)` parts of at most 10 s, as even as
/// possible, so no part is shorter than Omni's 3-second minimum unless the whole clip is.
pub fn omni_parts(duration: f64) -> Vec<(f64, f64)> {
  if !duration.is_finite() || duration <= 0.0 {
    return Vec::new();
  }
  let count = (duration / OMNI_MAX_SECONDS - 1e-6).ceil().max(1.0) as usize;
  let length = duration / count as f64;
  debug_assert!(count == 1 || length >= OMNI_MIN_SECONDS);
  (0..count).map(|i| (i as f64 * length, length)).collect()
}

/// A clip ready for Omni Flash: its parts as H.264 MP4s (at most 1920 px on the long side).
pub struct OmniSource {
  pub parts: Vec<(f64, f64, Vec<u8>)>,
  pub duration: f64,
}

pub fn prepare_for_omni(video: &Path) -> anyhow::Result<OmniSource> {
  anyhow::ensure!(video::ffmpeg_available(), "Describe edits need FFmpeg: install it with `brew install ffmpeg`.");
  let (_, _, _, duration) = video::probe_file(video).map_err(|e| anyhow::anyhow!("Couldn't read the clip: {e}"))?;
  anyhow::ensure!(duration > 0.2, "That clip is too short to edit");
  anyhow::ensure!(duration <= OMNI_LONGEST_CLIP + 0.5, "Describe edits take clips up to {OMNI_LONGEST_CLIP:.0} seconds; trim this one first (Trim tab)");
  let dir = tempfile::tempdir()?;
  let scale = "scale='min(1920,iw)':'min(1920,ih)':force_original_aspect_ratio=decrease,scale=trunc(iw/2)*2:trunc(ih/2)*2,setsar=1";
  let mut parts = Vec::new();
  for (i, (start, length)) in omni_parts(duration).into_iter().enumerate() {
    let out = dir.path().join(format!("part_{i}.mp4"));
    run_ffmpeg(&["-y", "-loglevel", "error", "-ss", &format!("{start:.3}"), "-i"], video, &["-t", &format!("{length:.3}"), "-vf", scale, "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p", "-c:a", "aac", "-movflags", "+faststart"], &out)?;
    parts.push((start, length, std::fs::read(&out)?));
  }
  Ok(OmniSource { parts, duration })
}

/// The prompt sent to Omni Flash: the user's words, plus Google's "keep everything else".
pub fn omni_prompt(prompt: &str) -> String {
  let prompt = prompt.trim();
  if prompt.to_ascii_lowercase().contains("keep everything else") {
    prompt.to_owned()
  } else {
    format!("{prompt}{}{KEEP_THE_REST}", if prompt.ends_with('.') { " " } else { ". " })
  }
}

/// Omni Flash's edited parts put back together: joined when there are several, then (with
/// `original`) the original clip's sound laid back over the picture.
pub fn assemble_omni(edited: Vec<Vec<u8>>, original: Option<&Path>) -> anyhow::Result<GeneratedFile> {
  let dir = tempfile::tempdir()?;
  let mut paths = Vec::new();
  for (i, bytes) in edited.iter().enumerate() {
    let path = dir.path().join(format!("edited_{i}.mp4"));
    std::fs::write(&path, bytes)?;
    paths.push(path);
  }
  let joined = match paths.as_slice() {
    [] => anyhow::bail!("Omni Flash returned no video"),
    [one] => one.clone(),
    many => {
      let path = dir.path().join("joined.mp4");
      std::fs::write(&path, join_videos(many)?.bytes)?;
      path
    },
  };
  let Some(original) = original.filter(|o| has_audio(o)) else {
    return Ok(GeneratedFile { bytes: std::fs::read(&joined)?, extension: "mp4", usage: None, via_key: false });
  };
  let out = dir.path().join("with_sound.mp4");
  let result = video::command("ffmpeg").args(["-y", "-loglevel", "error", "-i"]).arg(&joined).arg("-i").arg(original).args(["-map", "0:v:0", "-map", "1:a:0", "-c:v", "copy", "-c:a", "aac", "-shortest", "-movflags", "+faststart"]).arg(&out).stdout(Stdio::null()).stderr(Stdio::piped()).output()?;
  anyhow::ensure!(result.status.success() && out.exists(), "FFmpeg couldn't put the sound back: {}", String::from_utf8_lossy(&result.stderr).lines().last().unwrap_or("unknown error"));
  Ok(GeneratedFile { bytes: std::fs::read(&out)?, extension: "mp4", usage: None, via_key: false })
}

/// Gemini's request for an instruction edit: the image, the mask (if any), and the instruction.
pub fn instruct_parts(image: RefImage, mask: Option<&[u8]>, prompt: &str) -> (Vec<RefImage>, String) {
  match mask {
    Some(mask) => (vec![image, RefImage { mime_type: "image/png".into(), bytes: mask.to_vec() }], format!("{prompt}\n\n{MASK_INSTRUCTION}")),
    None => (vec![image], format!("{prompt}\n\nKeep everything else in the image exactly as it is.")),
  }
}

/// The InvokeModel body for a Stability image service.
pub fn stability_body(op: &EditOp, image: &[u8], mask: Option<&[u8]>) -> Value {
  let image = BASE64.encode(image);
  let mut body = match op {
    EditOp::Inpaint { prompt } => json!({ "image": image, "prompt": prompt }),
    EditOp::Erase => json!({ "image": image }),
    EditOp::SearchReplace { search, prompt } => json!({ "image": image, "search_prompt": search, "prompt": prompt }),
    EditOp::Outpaint { left, right, up, down, prompt } => {
      let mut b = json!({ "image": image, "left": left, "right": right, "up": up, "down": down });
      if !prompt.trim().is_empty() {
        b["prompt"] = json!(prompt);
      }
      b
    },
    EditOp::Instruct { .. } => json!({ "image": image }),
  };
  if let (Some(mask), EditOp::Inpaint { .. } | EditOp::Erase) = (mask, op) {
    body["mask"] = json!(BASE64.encode(mask));
  }
  body["output_format"] = json!("png");
  body
}

/// The request to send Gemini for an instruction edit.
pub fn gemini_edit<'a>(upstream: &'a str, images: &'a [RefImage], prompt: &'a str, image_size: Option<&'a str>) -> GeminiImageRequest<'a> {
  GeminiImageRequest { model: upstream, prompt, images, aspect_ratio: None, image_size }
}

/// The request to send Veo to extend a prepared clip.
pub fn veo_extend<'a>(upstream: &'a str, prompt: &'a str, clip: &'a [u8]) -> VeoRequest<'a> {
  VeoRequest { model: upstream, prompt, start_frame: None, end_frame: None, aspect_ratio: None, resolution: None, duration_seconds: None, sample_count: 1, generate_audio: Some(true), extend: Some(clip) }
}

fn has_audio(video: &Path) -> bool {
  video::command("ffprobe").args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=index", "-of", "csv=p=0"]).arg(video).output().is_ok_and(|o| o.status.success() && !o.stdout.is_empty())
}

fn run_ffmpeg(before_input: &[&str], input: &Path, after_input: &[&str], output: &Path) -> anyhow::Result<()> {
  let result = video::command("ffmpeg").args(before_input).arg(input).args(after_input).arg(output).stdout(Stdio::null()).stderr(Stdio::piped()).output()?;
  anyhow::ensure!(result.status.success() && output.exists(), "FFmpeg failed: {}", String::from_utf8_lossy(&result.stderr).lines().last().unwrap_or("unknown error"));
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Moving or removing a scene moves everything after it; each scene keeps its own length.
  #[test]
  fn a_films_scenes_run_end_to_end_in_their_current_order() {
    let scene = |name: &str, seconds: f64, still: bool| FilmScene { label: name.to_owned(), media: PathBuf::from(name), still, seconds, lyric: String::new() };
    let mut film = Film { title: "Adrift".into(), scenes: vec![scene("a.mp4", 2.5, false), scene("b.png", 4.0, true), scene("c.mp4", 1.5, false)], audio: None, size: (1280, 720), grade: Grade::None, fades: Fades::default() };
    let spans = |film: &Film| film.shots().iter().map(|shot| (shot.start, shot.end)).collect::<Vec<_>>();
    assert_eq!(spans(&film), [(0.0, 2.5), (2.5, 6.5), (6.5, 8.0)]);
    assert!(matches!(&film.shots()[1].media, ShotMedia::Still(path) if path == &PathBuf::from("b.png")));
    assert_eq!(film.seconds(), 8.0);
    film.scenes.swap(0, 2);
    assert_eq!(spans(&film), [(0.0, 1.5), (1.5, 5.5), (5.5, 8.0)]);
    film.scenes.remove(1);
    assert_eq!(spans(&film), [(0.0, 1.5), (1.5, 4.0)]);
    assert!(cut_film(&film).unwrap_err().to_string().contains("no soundtrack"));
    assert_eq!(ToolRequest::CutFilm { film }.estimate(&PriceBook::default()), 0.0);
  }

  /// A real cut: two scenes over a soundtrack come out as one film of their combined length,
  /// with sound.
  #[test]
  fn a_film_is_cut_from_its_scenes_over_the_soundtrack() {
    if !video::ffmpeg_available() {
      return;
    }
    let dir = tempfile::tempdir().unwrap();
    let make = |args: &[&str], name: &str| -> PathBuf {
      let path = dir.path().join(name);
      let done = video::command("ffmpeg").args(["-y", "-v", "error", "-f", "lavfi", "-i"]).args(args).arg(&path).status().unwrap();
      assert!(done.success(), "couldn't make {name}");
      path
    };
    let clip = make(&["testsrc=size=320x180:rate=24:duration=3", "-pix_fmt", "yuv420p"], "clip.mp4");
    let still = make(&["color=c=white:size=320x180", "-frames:v", "1"], "still.png");
    let song = make(&["sine=frequency=440:duration=6"], "song.wav");
    let scene = |media: &PathBuf, still: bool, seconds: f64| FilmScene { label: "s".into(), media: media.clone(), still, seconds, lyric: String::new() };
    let film = Film { title: "Adrift".into(), scenes: vec![scene(&clip, false, 2.0), scene(&still, true, 1.5)], audio: Some(song), size: (320, 180), grade: Grade::None, fades: Fades::default() };
    let cut = cut_film(&film).unwrap();
    let out = dir.path().join("film.mp4");
    std::fs::write(&out, &cut.bytes).unwrap();
    let (w, h, _, seconds) = video::probe_file(&out).unwrap();
    assert_eq!((w, h), (320, 180));
    assert!((seconds - 3.5).abs() < 0.1, "the film runs {seconds:.2}s");
    assert!(has_audio(&out), "the soundtrack is under it");
  }

  #[test]
  fn tools_are_priced_by_where_they_run() {
    let prices = PriceBook::default();
    let image = PathBuf::from("a.png");
    assert_eq!(ToolRequest::RemoveBackground { image: image.clone(), engine: BackgroundEngine::Local }.estimate(&prices), 0.0);
    assert_eq!(ToolRequest::RemoveBackground { image: image.clone(), engine: BackgroundEngine::Bedrock }.estimate(&prices), 0.07);
    assert_eq!(ToolRequest::EditImage { image: image.clone(), mask: None, op: EditOp::Outpaint { left: 100, right: 0, up: 0, down: 0, prompt: String::new() } }.estimate(&prices), 0.06);
    let extend = ToolRequest::ExtendVideo { video: PathBuf::from("a.mp4"), prompt: "go on".into(), tier: VeoTier::Fast };
    assert!((extend.estimate(&prices) - 0.70).abs() < 1e-9);
    let edit = ToolRequest::EditImage { image, mask: Some(vec![0]), op: EditOp::Instruct { pro: true, prompt: "x".into(), image_size: Some("one_k".into()) } };
    assert!(edit.estimate(&prices) > 0.134 && edit.estimate(&prices) < 0.14);
  }

  #[test]
  fn stability_bodies_carry_the_mask_only_where_it_applies() {
    let inpaint = stability_body(&EditOp::Inpaint { prompt: "a red door".into() }, &[1, 2, 3], Some(&[9]));
    assert_eq!(inpaint, json!({ "image": "AQID", "prompt": "a red door", "mask": "CQ==", "output_format": "png" }));
    let replace = stability_body(&EditOp::SearchReplace { search: "cat".into(), prompt: "dog".into() }, &[1, 2, 3], Some(&[9]));
    assert!(replace.get("mask").is_none());
    let out = stability_body(&EditOp::Outpaint { left: 200, right: 0, up: 0, down: 50, prompt: " ".into() }, &[1], None);
    assert_eq!((out["left"].as_u64(), out["down"].as_u64(), out.get("prompt")), (Some(200), Some(50), None));
  }

  #[test]
  fn masked_instructions_send_the_mask_second() {
    let image = RefImage { mime_type: "image/png".into(), bytes: vec![1] };
    let (images, prompt) = instruct_parts(image.clone(), Some(&[2]), "make the sky stormy");
    assert_eq!(images.len(), 2);
    assert!(prompt.starts_with("make the sky stormy") && prompt.contains("white in the mask"));
    assert_eq!(instruct_parts(image, None, "x").0.len(), 1);
  }

  #[test]
  fn describe_edits_are_priced_by_length_and_resolution() {
    let prices = PriceBook::default();
    let edit = |seconds: f64, resolution: &str| ToolRequest::DescribeEdit { video: PathBuf::from("a.mp4"), prompt: "remove the tail".into(), seconds, resolution: resolution.into(), keep_sound: true };
    assert!((edit(10.0, "720p").estimate(&prices) - prices.omni_edit_estimate(10.0, "720p")).abs() < 1e-12);
    assert!(edit(20.0, "720p").estimate(&prices) > edit(10.0, "720p").estimate(&prices));
    assert_eq!(edit(10.0, "720p").route().account, Some(Account::Vertex));
  }

  #[test]
  fn long_clips_are_cut_into_even_parts_of_ten_seconds_or_less() {
    assert_eq!(omni_parts(8.0), vec![(0.0, 8.0)]);
    assert_eq!(omni_parts(10.0), vec![(0.0, 10.0)], "exactly ten is one part");
    let parts = omni_parts(21.0);
    assert_eq!(parts.len(), 3);
    assert!(parts.iter().all(|(_, l)| (*l - 7.0).abs() < 1e-9), "even, not 10 + 10 + 1");
    assert!((parts[2].0 - 14.0).abs() < 1e-9);
    assert!(omni_parts(0.0).is_empty() && omni_parts(f64::NAN).is_empty());
  }

  #[test]
  fn prompts_ask_to_keep_everything_else() {
    assert_eq!(omni_prompt(" Remove the dog's tail "), "Remove the dog's tail. Keep everything else the same.");
    assert_eq!(omni_prompt("Remove the tail."), "Remove the tail. Keep everything else the same.");
    assert_eq!(omni_prompt("Remove it and keep everything else as is"), "Remove it and keep everything else as is");
  }

  /// Needs FFmpeg on the PATH: `cargo test -p gesso -- --ignored`.
  #[test]
  #[ignore]
  fn omni_parts_and_reassembly_with_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("long.mp4");
    let status = video::command("ffmpeg").args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc=s=640x360:d=13:r=30", "-f", "lavfi", "-i", "sine=duration=13", "-shortest", "-pix_fmt", "yuv420p"]).arg(&source).status().unwrap();
    assert!(status.success());
    let prepared = prepare_for_omni(&source).unwrap();
    assert_eq!(prepared.parts.len(), 2);
    // Stand-ins for Omni's edits: silent copies of each part.
    let edited: Vec<Vec<u8>> = prepared
      .parts
      .iter()
      .enumerate()
      .map(|(i, (_, _, bytes))| {
        let (input, output) = (dir.path().join(format!("in_{i}.mp4")), dir.path().join(format!("out_{i}.mp4")));
        std::fs::write(&input, bytes).unwrap();
        assert!(video::command("ffmpeg").args(["-y", "-loglevel", "error", "-i"]).arg(&input).args(["-an", "-c:v", "copy"]).arg(&output).status().unwrap().success());
        std::fs::read(&output).unwrap()
      })
      .collect();
    let result = assemble_omni(edited, Some(&source)).unwrap();
    let path = dir.path().join("result.mp4");
    std::fs::write(&path, &result.bytes).unwrap();
    let (w, h, _, duration) = video::probe_file(&path).unwrap();
    assert_eq!((w, h), (640, 360));
    assert!((duration - 13.0).abs() < 0.4, "{duration}");
    assert!(has_audio(&path), "the original sound is back");
  }

  /// Needs FFmpeg on the PATH: `cargo test -p gesso -- --ignored`.
  #[test]
  #[ignore]
  fn frames_trims_and_joins_with_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    let clip = |name: &str, color: &str| {
      let path = dir.path().join(name);
      let status = video::command("ffmpeg").args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", &format!("color=c={color}:s=320x240:d=3:r=24"), "-f", "lavfi", "-i", "sine=duration=3", "-shortest", "-pix_fmt", "yuv420p"]).arg(&path).status().unwrap();
      assert!(status.success());
      path
    };
    let red = clip("red.mp4", "red");
    let blue = clip("blue.mp4", "blue");
    let frames = extract_frames(&red, &[2.0, 0.5, 0.5]).unwrap();
    assert_eq!(frames.len(), 2, "duplicate times collapse");
    assert!(image::load_from_memory(&frames[0].bytes).is_ok());
    let trimmed = trim_video(&red, 0.5, 2.0).unwrap();
    let path = dir.path().join("trimmed.mp4");
    std::fs::write(&path, &trimmed.bytes).unwrap();
    let (_, _, _, duration) = video::probe_file(&path).unwrap();
    assert!((duration - 1.5).abs() < 0.2, "{duration}");
    let joined = join_videos(&[red, blue]).unwrap();
    let path = dir.path().join("joined.mp4");
    std::fs::write(&path, &joined.bytes).unwrap();
    let (w, h, _, duration) = video::probe_file(&path).unwrap();
    assert_eq!((w, h), (320, 240));
    assert!((duration - 6.0).abs() < 0.3, "{duration}");
  }
}
