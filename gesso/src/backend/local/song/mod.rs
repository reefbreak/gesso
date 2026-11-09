//! The Music Video page's work: an idea becomes lyrics, a song, a visual style, a brief, reference
//! images, scenes, clips and a finished film, one stage at a time, each on the user's own
//! accounts.
//!
//! | Stage            | What runs                                                              |
//! |------------------|------------------------------------------------------------------------|
//! | Lyrics           | A writer model: Gemini on Vertex, or Claude on Bedrock                 |
//! | Song             | Lyria 3 on Vertex (or a file the user brings)                          |
//! | Music Analysis   | Beats on the user's machine (FFmpeg); the read, sections and timed     |
//! |                  | lyrics by Gemini listening to the track                                |
//! | Visual Style     | The writer                                                             |
//! | Creative Brief   | The writer                                                             |
//! | Reference Images | Nano Banana: the performer, the venue and the props                    |
//! | Scenes           | Scene timing here ([`plan`]); the writer fills them; Nano Banana draws |
//! |                  | each opening frame from the reference images                           |
//! | Shots            | Veo, image-to-video from each still, silent                            |
//! | Final Video      | FFmpeg on the user's machine                                           |
//!
//! Every call is charged to the spend ledger as it happens. Stills and clips are kept in the
//! project's folder (a song has dozens); the song, the reference images and the finished video
//! also go into the library.

pub mod beats;
pub mod cut;
pub mod plan;
pub mod project;
pub mod writer;

use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::stream;
use log::{info, warn};
use serde_json::Value;

use super::bedrock::DEFAULT_CLAUDE_MODEL;
use super::fal::{FalClient, VOCALS_MODEL};
use super::library::{LocalLibrary, MediaClass, NewFile, Origin};
use super::spend::Account;
use super::tools::VeoTier;
use super::vertex::{GeminiImageRequest, SONG_CLIP_MODEL, TEXT_MODELS, TextRequest, VeoRequest};
use super::{GeneratedFile, LocalEngine, RefImage};
use crate::backend::Event;
use beats::BeatGrid;
use cut::{Aspect, CutShot, Fades, Format, Grade};
use plan::{Section, Slot, SongLength};
use project::{Quality, ReferenceKind};
use writer::{Brief, Design, Draft, PlannedShot, Read, Revision, Style, TimedLine, Turn};

/// Stills made at once (each is one Gemini call).
const STILLS_AT_ONCE: usize = 4;
/// Clips made at once (Veo's per-minute quota is small on new projects).
const CLIPS_AT_ONCE: usize = 3;
/// How often a call the provider turned away for quota is tried again, waiting 20 s, 40 s, 80 s
/// and then two minutes each time: about eight minutes of patience per picture or clip.
const QUOTA_RETRIES: u32 = 6;
const QUOTA_WAIT: Duration = Duration::from_secs(20);
const QUOTA_MAX_WAIT: Duration = Duration::from_secs(120);
/// Once a run has been turned away, its calls start at least this far apart, doubling each time
/// it happens again up to the longest gap.
const QUOTA_FIRST_GAP: Duration = Duration::from_secs(3);
const QUOTA_MAX_GAP: Duration = Duration::from_secs(24);
/// How often a waiting call looks at whether the run was stopped.
const STOP_CHECK: Duration = Duration::from_millis(400);
/// Draft stills are storyboard frames and Veo's starting images: 1K is plenty. Final stills are
/// redrawn at 2K, enough for 1080p clips.
const STILL_SIZE: &str = "1K";
const FINAL_STILL_SIZE: &str = "2K";
/// Scenes lip-synced at once (each is one fal job).
const SYNCS_AT_ONCE: usize = 3;
/// A still is drawn from at most this many reference images (the performer first).
const REFERENCES_PER_STILL: usize = 5;
/// The performer's sheet shows them three times side by side; props are shot square.
const LOOK_RATIO: &str = "16:9";
const PROP_RATIO: &str = "1:1";
/// The head shot fills a square.
const FACE_RATIO: &str = "1:1";
const NANO_BANANA: &str = "gemini-3.1-flash-image";
const NANO_BANANA_PRO: &str = "gemini-3-pro-image";
/// Veo makes 1080p only as 8-second clips.
const HD_CLIP_SECONDS: u16 = 8;
/// The page's grids show small copies of the pictures, this wide.
const THUMB_WIDTH: u32 = 480;
/// What the library records as the maker of the workflow's files.
const MODEL_ID: &str = "music_video";
const WRITING_TEMPERATURE: f32 = 1.0;
const LISTENING_TEMPERATURE: f32 = 0.1;

/// Which model writes the lyrics, the look and the shots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Writer {
  /// Gemini on Vertex AI.
  #[default]
  Gemini,
  /// Claude on Bedrock.
  Claude,
}

impl Writer {
  pub const ALL: [Writer; 2] = [Writer::Gemini, Writer::Claude];

  pub fn label(self) -> &'static str {
    match self {
      Writer::Gemini => "Gemini \u{b7} Vertex",
      Writer::Claude => "Claude \u{b7} Bedrock",
    }
  }
}

/// A reference image to make.
#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceJob {
  pub index: usize,
  pub version: u32,
  pub kind: ReferenceKind,
  pub name: String,
  pub description: String,
  /// Instead of drawing from the description: the picture to change, and how.
  pub change: Option<(PathBuf, String)>,
  /// The performer's other picture, so this one is the same person and not a new one: for the
  /// head shot, the sheet; for the sheet, the head shot it is to be drawn from.
  pub sheet: Option<PathBuf>,
}

/// A still to make.
#[derive(Clone, Debug, PartialEq)]
pub struct StillJob {
  pub index: usize,
  pub version: u32,
  pub image: String,
  /// The approved draft still to recreate at higher quality (the final pass).
  pub guide: Option<PathBuf>,
  /// This is the scene's ending frame, not its opening one (`image` is then its description).
  pub ending: bool,
  /// For an ending frame: the scene's opening still, so the two are one place, light and outfit.
  /// `None` when the same run draws that opening first; the run then fills it in.
  pub opening: Option<PathBuf>,
  /// The frame of this run (a scene and whether it is its ending) that this one is matched to
  /// and so waits for. An ending with no opening named waits for its own scene's opening.
  pub after: Option<(usize, bool)>,
  /// The performer sings this scene to the camera: the mouth must be in clear view, with
  /// nothing in front of it, or the lip sync fails.
  pub sing: bool,
}

/// A clip to make from a still.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipJob {
  pub index: usize,
  pub version: u32,
  pub still: PathBuf,
  /// The frame the clip must end on, for scenes that ask for one.
  pub end_still: Option<PathBuf>,
  pub motion: String,
  /// The slot's length; the clip is made at the shortest length Veo offers that covers it.
  pub seconds: f64,
  /// The performer sings in this scene: the clip is asked for with the mouth moving and in view,
  /// ready to be lip-synced.
  pub sing: bool,
}

/// A clip to lip-sync to the stretch of the song it plays over.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncJob {
  pub index: usize,
  pub version: u32,
  pub clip: PathBuf,
  pub start: f64,
  pub end: f64,
}

/// One stage's run. `dir` is the project's folder.
#[derive(Clone, Debug, PartialEq)]
pub enum SongRequest {
  /// A song from an idea, or a rewrite of `current` following `notes`.
  Write {
    idea: String,
    notes: String,
    current: Option<Draft>,
    /// How long a song to write for.
    length: SongLength,
    writer: Writer,
  },
  /// The song itself, from Lyria, asked for at `length` (30 seconds comes from the clip model).
  Compose {
    dir: PathBuf,
    draft: Draft,
    length: SongLength,
    instrumental: bool,
    version: u32,
  },
  /// A track the user made elsewhere.
  Import {
    dir: PathBuf,
    file: PathBuf,
  },
  /// The track's beats and sections.
  Analyse {
    audio: PathBuf,
    lyrics: String,
  },
  /// The look guiding every picture. `direction` is the user's own note on it.
  Style {
    idea: String,
    draft: Draft,
    read: Read,
    direction: String,
    writer: Writer,
  },
  /// The creative idea, the performer, the venue and the props.
  Brief {
    idea: String,
    draft: Draft,
    read: Read,
    style: Style,
    writer: Writer,
  },
  /// Pictures of the performer, the venue and the props, in the style's look.
  References {
    dir: PathBuf,
    look: String,
    aspect: Aspect,
    pro: bool,
    items: Vec<ReferenceJob>,
  },
  /// A picture and a movement for every slot. With `lip_sync`, the writer also marks the scenes
  /// the performer sings.
  Plan {
    brief: Brief,
    style: Style,
    slots: Vec<Slot>,
    lip_sync: bool,
    writer: Writer,
  },
  /// Makes a copy of the cut film sized for uploading to Instagram from a phone.
  Share {
    film: PathBuf,
  },
  /// An ending frame for each of `scenes`, which have none: a scene is its number, section,
  /// length, opening frame and movement.
  Endings {
    brief: Brief,
    scenes: Vec<(usize, String, f64, String, String)>,
    writer: Writer,
  },
  /// Each scene's opening frame, drawn from `references` (the performer's sheet first). The
  /// draft pass draws with Nano Banana 2 at 1K; the final pass redraws each approved draft still
  /// with Nano Banana Pro at 2K.
  Stills {
    dir: PathBuf,
    design: Design,
    references: Vec<(ReferenceKind, PathBuf)>,
    aspect: Aspect,
    quality: Quality,
    /// Draw with Nano Banana Pro whatever the pass: for draft stills the faster model got wrong.
    better: bool,
    shots: Vec<StillJob>,
  },
  Clips {
    dir: PathBuf,
    look: String,
    aspect: Aspect,
    tier: VeoTier,
    resolution: String,
    quality: Quality,
    shots: Vec<ClipJob>,
  },
  /// The next reply in a conversation about one section of the brief or the style.
  Workshop {
    context: String,
    section: String,
    fields: Vec<(String, String)>,
    turns: Vec<Turn>,
    writer: Writer,
  },
  /// Adds to the library whatever in the video's folder isn't `known` to be there yet.
  Catalogue {
    dir: PathBuf,
    known: BTreeSet<PathBuf>,
  },
  /// The user's own picture for a reference, copied into the project.
  ImportReference {
    dir: PathBuf,
    index: usize,
    version: u32,
    kind: ReferenceKind,
    file: PathBuf,
  },
  /// A vocal stem the user already has (exported from the app that made the song).
  ImportVocals {
    dir: PathBuf,
    file: PathBuf,
  },
  /// Redraws the mouth in each sung scene's clip to match the vocal, on fal. Without `vocals`,
  /// the vocal stem is separated from `track` first.
  LipSync {
    dir: PathBuf,
    track: PathBuf,
    vocals: Option<PathBuf>,
    quality: Quality,
    shots: Vec<SyncJob>,
  },
  Cut {
    dir: PathBuf,
    audio: PathBuf,
    aspect: Aspect,
    resolution: String,
    grade: Grade,
    fades: Fades,
    format: Format,
    shots: Vec<CutShot>,
    version: u32,
  },
}

impl SongRequest {
  /// Which part of a video's cost the request's charges belong to (one of [`COST_PARTS`]).
  pub fn cost_part(&self) -> &'static str {
    match self {
      SongRequest::Compose { .. } => COST_PARTS[1],
      SongRequest::References { .. } => COST_PARTS[2],
      SongRequest::Stills { .. } => COST_PARTS[3],
      SongRequest::Clips { .. } => COST_PARTS[4],
      SongRequest::LipSync { .. } => COST_PARTS[5],
      _ => COST_PARTS[0],
    }
  }
}

/// What a run reports back, as it goes. Every run ends with `Done`.
#[derive(Debug)]
pub enum SongUpdate {
  Written(Draft),
  Track {
    path: PathBuf,
    model: String,
    sung: String,
  },
  Analysed {
    grid: BeatGrid,
    sections: Vec<Section>,
    read: Read,
    lines: Vec<TimedLine>,
    note: Option<String>,
  },
  Styled(Style),
  Briefed(Brief),
  /// The writer's reply about one section.
  Workshopped(Revision),
  Reference {
    index: usize,
    path: PathBuf,
  },
  ReferenceFailed {
    index: usize,
    message: String,
  },
  Planned(Vec<PlannedShot>),
  /// The film's Instagram copy: where it is, the cut it was made from, the bitrate it was
  /// encoded for and its size.
  Shared {
    path: PathBuf,
    of: PathBuf,
    kbps: u32,
    bytes: u64,
  },
  /// Ending frames for scenes that had none, by scene.
  Endings(Vec<(usize, String)>),
  Still {
    index: usize,
    path: PathBuf,
    /// The scene's ending frame, not its opening one.
    ending: bool,
  },
  Clip {
    index: usize,
    path: PathBuf,
  },
  /// The track's vocal stem.
  Vocals(PathBuf),
  /// A scene's clip, lip-synced.
  Synced {
    index: usize,
    path: PathBuf,
  },
  ShotFailed {
    index: usize,
    message: String,
  },
  Cut(PathBuf),
  /// The video's files that were just added to the library.
  Catalogued(Vec<PathBuf>),
  /// A progress line for the page.
  Note(String),
  Done(Result<(), String>),
}

/// What a sung scene's frames must show for the lip sync to take: a clear, unobstructed mouth.
const SUNG_FRAME: &str = "The performer sings this scene to the camera, so the mouth must be in clear view: face towards the lens, the mouth and chin fully visible, lit and in sharp focus, with nothing at all in front of or touching the lower face. No microphone anywhere near the mouth (no handheld mic, no mic stand in front of the face, no headset), no hands, hair, cup, bottle, cigarette, smoke, scarf, mask or prop in the way, and no one between the camera and the performer.";

/// How many scenes one call to the writer is asked to give ending frames.
const ENDINGS_PER_CALL: usize = 30;

/// What a video's cost is made of, in the order it's shown.
pub const COST_PARTS: [&str; 6] = ["Writing and planning", "Song", "Reference images", "Scene images", "Clips", "Lip sync"];

/// Something the page prices before running it.
#[derive(Clone, Debug, PartialEq)]
pub enum SongEstimate {
  Song {
    clip: bool,
  },
  /// Reference images or stills: `count` pictures. `large` is the final pass's 2K.
  Stills {
    count: usize,
    pro: bool,
    large: bool,
  },
  /// Lip sync for `seconds` of video, plus separating the vocal from a track of
  /// `separate_seconds` when that hasn't been done.
  LipSync {
    seconds: f64,
    pro: bool,
    separate_seconds: f64,
  },
  /// `seconds` of video in all (see [`project::SongProject::seconds_to_generate`]).
  Clips {
    seconds: u32,
    tier: VeoTier,
    resolution: String,
  },
}

impl LocalEngine {
  /// Runs a stage in the background; the page hears back by `run_id`.
  #[cfg(test)]
  pub fn start_song(self: &Arc<Self>, run_id: u64, request: SongRequest, tx: Sender<Event>, ctx: egui::Context) {
    self.start_song_for(run_id, None, request, tx, ctx);
  }

  /// As [`Self::start_song`], with the run's charges counted towards `project` (its id).
  pub fn start_song_for(self: &Arc<Self>, run_id: u64, project: Option<String>, request: SongRequest, tx: Sender<Event>, ctx: egui::Context) {
    let part = request.cost_part();
    let cancel = Arc::new(AtomicBool::new(false));
    if let Ok(mut runs) = self.song_runs.lock() {
      runs.insert(run_id, cancel.clone());
    }
    let engine = self.clone();
    tokio::spawn(async move {
      let run = SongRun { engine: &engine, run_id, tx: &tx, ctx: &ctx, cancel, batch: LocalLibrary::new_batch_token(), pace: Pace::new(QUOTA_WAIT), project, part };
      let result = run.stage(request).await.map_err(|err| {
        warn!("Music video run {run_id} failed: {err:#}");
        format!("{err:#}")
      });
      run.send(SongUpdate::Done(result));
      if let Ok(mut runs) = engine.song_runs.lock() {
        runs.remove(&run_id);
      }
    });
  }

  /// Stops a run before its next still or clip; calls already in flight finish (and are billed).
  pub fn cancel_song(&self, run_id: u64) {
    if let Some(cancel) = self.song_runs.lock().ok().and_then(|runs| runs.get(&run_id).cloned()) {
      cancel.store(true, Ordering::Relaxed);
    }
  }

  /// What a stage would cost at list prices.
  pub fn estimate_song(&self, estimate: &SongEstimate) -> f64 {
    match estimate {
      SongEstimate::Song { clip } => self.prices.lyria_song(if *clip { SONG_CLIP_MODEL } else { self.vertex.song_models()[0] }),
      SongEstimate::Stills { count, pro, large } => *count as f64 * self.prices.gemini_call(still_model(*pro), None, Some(if *large { FINAL_STILL_SIZE } else { STILL_SIZE })).unwrap_or(0.0),
      SongEstimate::LipSync { seconds, pro, separate_seconds } => seconds * self.prices.lip_sync_per_second(*pro) + separate_seconds * self.prices.vocals_per_second(),
      SongEstimate::Clips { seconds, tier, resolution } => f64::from(*seconds) * self.prices.veo_per_second(tier.model(), false, resolution).unwrap_or(0.0),
    }
  }
}

/// One run: the engine, where to report, and whether it's been stopped.
struct SongRun<'a> {
  engine: &'a LocalEngine,
  run_id: u64,
  tx: &'a Sender<Event>,
  ctx: &'a egui::Context,
  cancel: Arc<AtomicBool>,
  /// Groups the run's charges in the spend ledger.
  batch: String,
  /// Slows the whole run down once a provider's per-minute limit is met.
  pace: Pace,
  /// The video the run's charges count towards, and which part of it.
  project: Option<String>,
  part: &'static str,
}

impl SongRun<'_> {
  async fn stage(&self, request: SongRequest) -> anyhow::Result<()> {
    match request {
      SongRequest::Write { idea, notes, current, length, writer } => {
        let reply = self.write(writer, writer::SONGWRITER, &writer::song_prompt(&idea, &notes, current.as_ref(), length)).await?;
        self.send(SongUpdate::Written(writer::parse_draft(&reply)?));
      },
      SongRequest::Compose { dir, draft, length, instrumental, version } => self.compose(&dir, &draft, length, instrumental, version).await?,
      SongRequest::Import { dir, file } => {
        let extension = file.extension().and_then(|e| e.to_str()).unwrap_or("mp3").to_ascii_lowercase();
        let path = dir.join(format!("song_imported.{extension}"));
        std::fs::create_dir_all(&dir)?;
        std::fs::copy(&file, &path).map_err(|e| anyhow::anyhow!("Couldn't copy {}: {e}", file.display()))?;
        self.add_to_library(&path, &std::fs::read(&path)?, MediaClass::Audio);
        self.send(SongUpdate::Track { path, model: String::new(), sung: String::new() });
      },
      SongRequest::Analyse { audio, lyrics } => self.analyse(&audio, &lyrics).await?,
      SongRequest::Style { idea, draft, read, direction, writer } => {
        let reply = self.write(writer, writer::DIRECTOR, &writer::style_prompt(&idea, &draft, &read, &direction)).await?;
        self.send(SongUpdate::Styled(writer::parse_style(&reply)?));
      },
      SongRequest::Brief { idea, draft, read, style, writer } => {
        let reply = self.write(writer, writer::DIRECTOR, &writer::brief_prompt(&idea, &draft, &read, &style)).await?;
        self.send(SongUpdate::Briefed(writer::parse_brief(&reply)?));
      },
      SongRequest::Workshop { context, section, fields, turns, writer } => {
        let reply = self.write(writer, writer::EDITOR, &writer::workshop_prompt(&context, &section, &fields, &turns)).await?;
        self.send(SongUpdate::Workshopped(writer::parse_workshop(&reply, &fields)));
      },
      SongRequest::References { dir, look, aspect, pro, items } => {
        let (dir, look) = (&dir.join("references"), look.as_str());
        let jobs = items.into_iter().map(|job| async move {
          if self.stopped() {
            return;
          }
          let index = job.index;
          match self.reference(dir, look, aspect, pro, job).await {
            Ok(path) => self.send(SongUpdate::Reference { index, path }),
            Err(err) => self.send(SongUpdate::ReferenceFailed { index, message: format!("{err:#}") }),
          }
        });
        stream::iter(jobs).buffer_unordered(STILLS_AT_ONCE).collect::<Vec<()>>().await;
      },
      SongRequest::Plan { brief, style, slots, lip_sync, writer } => {
        let reply = self.write(writer, writer::DIRECTOR, &writer::scenes_prompt(&brief, &style, &slots, lip_sync)).await?;
        let written = writer::parse_scenes(&reply);
        let missing = slots.iter().filter(|slot| !written.iter().any(|scene| scene.index == slot.index)).count();
        if missing > 0 {
          self.send(SongUpdate::Note(format!("The writer left out {missing} of {} scenes; they have stand-in descriptions you can edit.", slots.len())));
        }
        // Every scene ends somewhere: one the writer gave no ending frame gets its movement's.
        let scenes = slots.iter().map(|slot| written.iter().find(|scene| scene.index == slot.index).cloned().unwrap_or_else(|| writer::fallback_scene(slot))).map(|scene| if scene.ending.is_empty() { PlannedShot { ending: writer::fallback_ending(&scene.image, &scene.motion), ..scene } } else { scene }).collect();
        self.send(SongUpdate::Planned(scenes));
      },
      SongRequest::Endings { brief, scenes, writer } => {
        // A long film is asked for in parts, so no reply is cut short; a failed part falls back
        // to each scene's own movement and the rest carry on.
        let mut endings = Vec::new();
        for part in scenes.chunks(ENDINGS_PER_CALL) {
          let written = match self.write(writer, writer::DIRECTOR, &writer::endings_prompt(&brief, part)).await {
            Ok(reply) => writer::parse_endings(&reply),
            Err(err) => {
              warn!("Writing ending frames failed: {err:#}");
              Vec::new()
            },
          };
          endings.extend(part.iter().map(|(index, _, _, image, motion)| (*index, written.iter().find(|(i, _)| i == index).map_or_else(|| writer::fallback_ending(image, motion), |(_, ending)| ending.clone()))));
          self.send(SongUpdate::Note(format!("Writing ending frames\u{2026} {} of {}", endings.len(), scenes.len())));
        }
        self.send(SongUpdate::Endings(endings));
      },
      SongRequest::Stills { dir, design, references, aspect, quality, better, shots } => {
        let references: Vec<(ReferenceKind, RefImage)> = references.iter().take(REFERENCES_PER_STILL).map(|(kind, path)| Ok((*kind, self.engine.load_path(path)?))).collect::<anyhow::Result<_>>()?;
        let (dir, design, references) = (&dir.join("shots"), &design, &references);
        // A frame that is matched to another frame of this run waits for it: an ending for its
        // scene's opening, and in a flowing sequence for the ending of the scene before, which
        // is its opening. So the run goes in waves, each drawing what the last made possible.
        let mut waiting: Vec<StillJob> = shots.into_iter().map(|job| if job.ending && job.opening.is_none() && job.after.is_none() { StillJob { after: Some((job.index, false)), ..job } } else { job }).collect();
        let mut drawn: Vec<((usize, bool), PathBuf)> = Vec::new();
        let draw = |job: StillJob| async move {
          if self.stopped() {
            return None;
          }
          let (index, ending) = (job.index, job.ending);
          match self.still(dir, design, references, aspect, quality, better, job).await {
            Ok(path) => {
              self.send(SongUpdate::Still { index, path: path.clone(), ending });
              Some(((index, ending), path))
            },
            Err(err) => {
              self.send(SongUpdate::ShotFailed { index, message: format!("{err:#}") });
              None
            },
          }
        };
        loop {
          let (ready, rest): (Vec<StillJob>, Vec<StillJob>) = waiting.into_iter().partition(|job| job.after.is_none_or(|frame| drawn.iter().any(|(made, _)| *made == frame)));
          // What is left waits on a frame that failed or was stopped: nothing to match it to.
          if ready.is_empty() {
            break;
          }
          waiting = rest;
          let ready: Vec<StillJob> = ready.into_iter().map(|job| match job.after.and_then(|frame| drawn.iter().find(|(made, _)| *made == frame)) {
            Some((_, path)) => StillJob { opening: Some(path.clone()), after: None, ..job },
            None => job,
          }).collect();
          drawn.extend(stream::iter(ready.into_iter().map(draw)).buffer_unordered(STILLS_AT_ONCE).collect::<Vec<_>>().await.into_iter().flatten());
        }
      },
      SongRequest::Clips { dir, look, aspect, tier, resolution, quality, shots } => {
        let (dir, look, resolution) = (&dir.join("clips"), look.as_str(), resolution.as_str());
        let jobs = shots.into_iter().map(|job| async move {
          if self.stopped() {
            return;
          }
          let index = job.index;
          match self.clip(dir, look, aspect, tier, resolution, quality, job).await {
            Ok(path) => self.send(SongUpdate::Clip { index, path }),
            Err(err) => self.send(SongUpdate::ShotFailed { index, message: format!("{err:#}") }),
          }
        });
        stream::iter(jobs).buffer_unordered(CLIPS_AT_ONCE).collect::<Vec<()>>().await;
      },
      SongRequest::Catalogue { dir, known } => self.catalogue(&dir, &known),
      SongRequest::ImportReference { dir, index, version, kind, file } => {
        let bytes = std::fs::read(&file).map_err(|e| anyhow::anyhow!("Couldn't read {}: {e}", file.display()))?;
        // Re-encoded as PNG: the image models take PNG, JPEG and WebP, and uploads come in more kinds.
        let png = tokio::task::block_in_place(|| -> anyhow::Result<Vec<u8>> {
          let image = image::load_from_memory(&bytes).map_err(|e| anyhow::anyhow!("{} isn't a picture Gesso can read: {e}", file.display()))?;
          let mut out = Vec::new();
          image.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
          Ok(out)
        })?;
        let path = save_picture(&dir.join("references"), &format!("{}_{:02}_v{version}.png", kind.label(), index + 1), &png)?;
        self.add_to_library(&path, &png, MediaClass::Image);
        self.send(SongUpdate::Reference { index, path });
      },
      SongRequest::ImportVocals { dir, file } => {
        let extension = file.extension().and_then(|e| e.to_str()).unwrap_or("mp3").to_ascii_lowercase();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("vocals_imported.{extension}"));
        std::fs::copy(&file, &path).map_err(|e| anyhow::anyhow!("Couldn't copy {}: {e}", file.display()))?;
        self.add_to_library(&path, &std::fs::read(&path)?, MediaClass::Audio);
        self.send(SongUpdate::Vocals(path));
      },
      SongRequest::LipSync { dir, track, vocals, quality, shots } => {
        let fal = self.engine.fal()?;
        let vocals = match vocals {
          Some(path) => path,
          None => self.separate_vocals(&fal, &dir, &track).await?,
        };
        let (dir, fal, vocals) = (&dir.join("clips"), &fal, vocals.as_path());
        let jobs = shots.into_iter().map(|job| async move {
          if self.stopped() {
            return;
          }
          let index = job.index;
          match self.sync(fal, dir, vocals, quality, job).await {
            Ok(path) => self.send(SongUpdate::Synced { index, path }),
            Err(err) => self.send(SongUpdate::ShotFailed { index, message: format!("{err:#}") }),
          }
        });
        stream::iter(jobs).buffer_unordered(SYNCS_AT_ONCE).collect::<Vec<()>>().await;
      },
      SongRequest::Cut { dir, audio, aspect, resolution, grade, fades, format, shots, version } => {
        std::fs::create_dir_all(&dir)?;
        let out = dir.join(format!("cut_v{version}.mp4"));
        let target = out.clone();
        tokio::task::spawn_blocking(move || cut::assemble(&shots, &audio, format.size(aspect, &resolution), grade, fades, &target)).await??;
        let bytes = std::fs::read(&out)?;
        self.add_to_library(&out, &bytes, MediaClass::Video);
        self.send(SongUpdate::Cut(out));
      },
      SongRequest::Share { film } => {
        let out = film.with_file_name(format!("{}_instagram.mp4", film.file_stem().unwrap_or_default().to_string_lossy()));
        let (source, target) = (film.clone(), out.clone());
        let kbps = tokio::task::spawn_blocking(move || cut::share_copy(&source, &target)).await??;
        let bytes = std::fs::read(&out)?;
        self.add_to_library(&out, &bytes, MediaClass::Video);
        self.send(SongUpdate::Shared { path: out, of: film, kbps, bytes: bytes.len() as u64 });
      },
    }
    Ok(())
  }

  async fn compose(&self, dir: &Path, draft: &Draft, length: SongLength, instrumental: bool, version: u32) -> anyhow::Result<()> {
    let configured = self.engine.config.song_music_model.as_deref();
    // The 30-second piece has a model of its own; every other length is asked of the song model.
    let models: Vec<&str> = match (length.is_clip(), configured) {
      (true, _) => vec![SONG_CLIP_MODEL],
      (false, Some(model)) => vec![model],
      (false, None) => self.engine.vertex.song_models().to_vec(),
    };
    // Google's filter sometimes refuses a prompt it accepts a moment later, and is stricter with
    // the direction than with the lyrics. A refusal costs nothing, so ask again: the same way
    // once, then with the directions found in the lyrics left out, then with the lyrics as typed.
    let mut tried: Vec<String> = Vec::new();
    let mut song = None;
    let mut refusal = None;
    for wording in [Wording::Sorted, Wording::Sorted, Wording::WithoutNotes, Wording::AsTyped] {
      let prompt = music_prompt(draft, length, instrumental, wording);
      if tried.iter().filter(|earlier| **earlier == prompt).count() >= if wording == Wording::Sorted { 2 } else { 1 } {
        continue;
      }
      if let Some(note) = wording.note().filter(|_| !tried.is_empty()) {
        self.send(SongUpdate::Note(note.to_owned()));
      }
      tried.push(prompt.clone());
      info!("Asking for the song ({wording:?}) with:\n{prompt}");
      match self.engine.vertex.generate_song(&models, &prompt).await {
        Ok(made) => {
          song = Some(made);
          break;
        },
        Err(err) if is_blocked(&err) && !self.stopped() => {
          warn!("The song's prompt was refused ({wording:?}): {err:#}");
          refusal = Some(err);
        },
        Err(err) => return Err(err),
      }
    }
    let song = match (song, refusal) {
      (Some(song), _) => song,
      (None, Some(err)) => return Err(err.context(format!("Google's filter refused the song {} times, however it was worded. It doesn't say which words it objects to: look for an artist, band or song name, or violent or explicit wording, in the music direction and the lyrics", tried.len()))),
      (None, None) => anyhow::bail!("The song wasn't made"),
    };
    info!("Song made by {}", song.model);
    self.charge(google_account(&song.file), self.engine.prices.lyria_song(&song.model), &song.model);
    let path = save(dir, &format!("song_v{version}.{}", song.file.extension), &song.file.bytes)?;
    self.add_to_library(&path, &song.file.bytes, MediaClass::Audio);
    self.send(SongUpdate::Track { path, model: song.model, sung: song.text });
    Ok(())
  }

  /// Beats on this machine; sections from Gemini listening to the track, or guessed from the
  /// lyrics when it can't.
  async fn analyse(&self, audio: &Path, lyrics: &str) -> anyhow::Result<()> {
    let track = audio.to_path_buf();
    let (duration, beats, copy) = tokio::task::spawn_blocking(move || (cut::duration(&track), beats::analyse_file(&track), cut::listening_copy(&track))).await?;
    let duration = duration?;
    let mut notes: Vec<String> = Vec::new();
    let grid = beats.unwrap_or_else(|err| {
      warn!("Beat analysis failed: {err:#}");
      notes.push("No steady beat was found, so cuts fall on an even 120 BPM grid.".to_owned());
      BeatGrid::steady(120.0, duration)
    });
    // Sections are named from the tags, so a direction written into one mustn't become its name.
    let lyrics = plan::tidy_lyrics(lyrics).lyrics;
    let lyrics = lyrics.as_str();
    let heard = match copy {
      Ok((bytes, mime)) if self.engine.config.vertex_enabled() => {
        self.send(SongUpdate::Note("Gemini is listening to the track\u{2026}".to_owned()));
        match self.text(Writer::Gemini, writer::LISTENER, &writer::listen_prompt(lyrics, duration), Some((mime, &bytes)), LISTENING_TEMPERATURE).await {
          Ok(reply) => writer::parse_heard(&reply),
          Err(err) => {
            warn!("Listening to the track failed: {err:#}");
            notes.push(format!("Gemini couldn't listen to the track ({err:#})."));
            writer::Heard::default()
          },
        }
      },
      _ => writer::Heard::default(),
    };
    let sections = if heard.sections.is_empty() {
      notes.push("The sections are estimated from the lyrics, not heard; check them against the track.".to_owned());
      plan::sections_from_lyrics(lyrics, duration)
    } else {
      heard.sections
    };
    self.send(SongUpdate::Analysed { grid, sections, read: heard.read, lines: heard.lines, note: (!notes.is_empty()).then(|| notes.join(" ")) });
    Ok(())
  }

  async fn reference(&self, dir: &Path, look: &str, aspect: Aspect, pro: bool, job: ReferenceJob) -> anyhow::Result<PathBuf> {
    let model = still_model(pro);
    // One picture goes with the request at most: the one to change, or the performer's other
    // picture (the sheet a head shot is drawn from, or the head shot a sheet is).
    let likeness = matches!(job.kind, ReferenceKind::Face | ReferenceKind::Look) && job.change.is_none() && job.sheet.is_some();
    let picture = job.change.as_ref().map(|(path, _)| path).or(job.sheet.as_ref().filter(|_| likeness)).map(|path| self.engine.load_path(path)).transpose()?;
    let prompt = match &job.change {
      Some((_, change)) => change_prompt(change),
      None if likeness && job.kind == ReferenceKind::Face => face_from_sheet_prompt(look),
      None if likeness => sheet_from_face_prompt(&job.name, &job.description, look),
      None => reference_prompt(job.kind, &job.name, &job.description, look),
    };
    let images: Vec<RefImage> = picture.into_iter().collect();
    let ratio = match job.kind {
      ReferenceKind::Look => LOOK_RATIO,
      ReferenceKind::Face => FACE_RATIO,
      ReferenceKind::Location => aspect.ratio(),
      ReferenceKind::Prop => PROP_RATIO,
    };
    let file = self.patiently(|| self.engine.vertex.generate_image(GeminiImageRequest { model, prompt: &prompt, images: &images, aspect_ratio: Some(ratio), image_size: Some(STILL_SIZE) })).await?;
    self.charge(google_account(&file), self.engine.prices.gemini_call(model, file.usage, Some(STILL_SIZE)).unwrap_or(0.0), model);
    let file = in_keeping(file, look);
    let path = save_picture(dir, &format!("{}_{:02}_v{}.{}", job.kind.label(), job.index + 1, job.version, file.extension), &file.bytes)?;
    self.add_to_library(&path, &file.bytes, MediaClass::Image);
    Ok(path)
  }

  #[allow(clippy::too_many_arguments)]
  async fn still(&self, dir: &Path, design: &Design, references: &[(ReferenceKind, RefImage)], aspect: Aspect, quality: Quality, better: bool, job: StillJob) -> anyhow::Result<PathBuf> {
    let (model, size) = match quality {
      Quality::Draft => (still_model(better), STILL_SIZE),
      Quality::Final => (still_model(true), FINAL_STILL_SIZE),
    };
    // The final pass recreates the approved draft frame: it goes first, ahead of the references.
    let guide = job.guide.as_deref().map(|path| self.engine.load_path(path)).transpose()?;
    // An ending frame is drawn with its scene's opening frame beside it, after the draft guide.
    let opening = job.opening.as_deref().filter(|_| job.ending).map(|path| self.engine.load_path(path)).transpose()?;
    let kinds: Vec<ReferenceKind> = references.iter().map(|(kind, _)| *kind).collect();
    let prompt = still_prompt(design, &job.image, &kinds, guide.is_some(), opening.is_some(), job.sing);
    let images: Vec<RefImage> = guide.into_iter().chain(opening).chain(references.iter().map(|(_, image)| image.clone())).collect();
    let file = self.patiently(|| self.engine.vertex.generate_image(GeminiImageRequest { model, prompt: &prompt, images: &images, aspect_ratio: Some(aspect.ratio()), image_size: Some(size) })).await?;
    self.charge(google_account(&file), self.engine.prices.gemini_call(model, file.usage, Some(size)).unwrap_or(0.0), model);
    let file = in_keeping(file, &design.look);
    let path = save_picture(dir, &format!("shot_{:03}_{}{}_v{}.{}", job.index + 1, quality.tag(), if job.ending { "_end" } else { "" }, job.version, file.extension), &file.bytes)?;
    self.add_to_library(&path, &file.bytes, MediaClass::Image);
    Ok(path)
  }

  #[allow(clippy::too_many_arguments)]
  async fn clip(&self, dir: &Path, look: &str, aspect: Aspect, tier: VeoTier, resolution: &str, quality: Quality, job: ClipJob) -> anyhow::Result<PathBuf> {
    let still = self.engine.load_path(&job.still)?;
    // With an ending frame Veo is given both ends of the shot and fills in the move between.
    let end = job.end_still.as_deref().map(|path| self.engine.load_path(path)).transpose()?;
    let seconds = if resolution == "1080p" { HD_CLIP_SECONDS } else { plan::clip_seconds(job.seconds) };
    let prompt = clip_prompt(look, &job.motion, job.sing, end.is_some());
    let files = self.patiently(|| self.engine.vertex.generate_video(VeoRequest { model: tier.model(), prompt: &prompt, start_frame: Some(&still), end_frame: end.as_ref(), aspect_ratio: Some(aspect.ratio()), resolution: Some(resolution), duration_seconds: Some(seconds), sample_count: 1, generate_audio: Some(false), extend: None })).await?;
    let rate = self.engine.prices.veo_per_second(tier.model(), false, resolution).unwrap_or(0.0);
    self.charge(Account::Vertex, f64::from(seconds) * rate, tier.model());
    let file = files.into_iter().next().ok_or_else(|| anyhow::anyhow!("Veo returned no video"))?;
    let path = save(dir, &format!("clip_{:03}_{}_v{}.mp4", job.index + 1, quality.tag(), job.version), &file.bytes)?;
    self.add_to_library(&path, &file.bytes, MediaClass::Video);
    Ok(path)
  }

  /// The track's vocal stem, separated on fal and kept in the project for every later sync.
  async fn separate_vocals(&self, fal: &FalClient, dir: &Path, track: &Path) -> anyhow::Result<PathBuf> {
    self.send(SongUpdate::Note("Separating the vocal from the track\u{2026}".to_owned()));
    let source = track.to_path_buf();
    let (seconds, copy) = tokio::task::spawn_blocking(move || (cut::duration(&source), cut::separation_copy(&source))).await?;
    let vocals = fal.vocals(&copy?, "audio/wav").await?;
    self.charge(Account::Fal, seconds.unwrap_or(0.0) * self.engine.prices.vocals_per_second(), VOCALS_MODEL);
    let path = save(dir, "vocals.mp3", &vocals)?;
    self.add_to_library(&path, &vocals, MediaClass::Audio);
    self.send(SongUpdate::Vocals(path.clone()));
    Ok(path)
  }

  /// One scene: the part of the clip the cut uses, and the vocal under it, synced on fal.
  async fn sync(&self, fal: &FalClient, dir: &Path, vocals: &Path, quality: Quality, job: SyncJob) -> anyhow::Result<PathBuf> {
    let (clip, stem, seconds) = (job.clip.clone(), vocals.to_path_buf(), job.end - job.start);
    let (start, end) = (job.start, job.end);
    let (head, voice) = tokio::task::spawn_blocking(move || (cut::head_of_clip(&clip, seconds), cut::slice_of_track(&stem, start, end))).await?;
    let pro = quality == Quality::Final;
    let voice = voice?;
    // The slice is kept beside the synced clip, so a sync that lands at the wrong moment can be
    // checked against the sound it was given: if the slice is right, the model is to blame.
    save(dir, &format!("clip_{:03}_{}_voice_v{}.wav", job.index + 1, quality.tag(), job.version), &voice)?;
    let synced = fal.lip_sync(&head?, &voice, pro).await?;
    self.charge(Account::Fal, seconds * self.engine.prices.lip_sync_per_second(pro), if pro { "lipsync-2-pro" } else { "lipsync-2" });
    let path = save(dir, &format!("clip_{:03}_{}_synced_v{}.mp4", job.index + 1, quality.tag(), job.version), &synced)?;
    self.add_to_library(&path, &synced, MediaClass::Video);
    Ok(path)
  }

  /// Asks the writer for JSON.
  async fn write(&self, writer: Writer, system: &str, prompt: &str) -> anyhow::Result<String> {
    self.text(writer, system, prompt, None, WRITING_TEMPERATURE).await
  }

  /// One text call, charged to whichever account ran it.
  async fn text(&self, writer: Writer, system: &str, prompt: &str, audio: Option<(&str, &[u8])>, temperature: f32) -> anyhow::Result<String> {
    let config = &self.engine.config;
    match writer {
      Writer::Claude => {
        anyhow::ensure!(config.bedrock_enabled(), "Claude runs on Bedrock, which isn't set up: no AWS credentials were found. Pick Gemini, or sign in to AWS.");
        let model = config.song_claude_model.as_deref().unwrap_or(DEFAULT_CLAUDE_MODEL);
        let reply = self.engine.bedrock.converse(model, system, prompt, temperature).await?;
        self.charge(Account::Bedrock, self.engine.prices.text_call(model, reply.input_tokens, reply.output_tokens), model);
        Ok(reply.text)
      },
      Writer::Gemini => {
        let models: Vec<&str> = config.song_text_model.as_deref().map_or_else(|| TEXT_MODELS.to_vec(), |model| vec![model]);
        let reply = self.engine.vertex.generate_text(TextRequest { models: &models, system, prompt, audio, json: true, temperature }).await?;
        let usage = reply.usage.unwrap_or_default();
        self.charge(Account::Vertex, self.engine.prices.text_call(&reply.model, usage.prompt_tokens, usage.candidates_tokens + usage.thoughts_tokens), &reply.model);
        Ok(reply.text)
      },
    }
  }

  /// Puts a copy of one of the video's files (at `path`, holding `bytes`) in the library, titled
  /// with the video's name and what the file is. Every version of every asset goes in.
  fn add_to_library(&self, path: &Path, bytes: &[u8], class: MediaClass) {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    let title = asset_title(path);
    // Filed with what the app made, even a track or picture the user brought in: the library
    // page leaves uploads out, and every one of a video's files should be found there. The
    // title says which were imported.
    let added = tokio::task::block_in_place(|| self.engine.library.add(NewFile { bytes, extension, class, origin: Origin::Generated, maybe_batch_token: Some(&self.batch), maybe_prompt_token: None, maybe_model_id: Some(MODEL_ID), maybe_title: Some(&title) }));
    if let Err(err) = added {
      warn!("Couldn't add {} to the library: {err:#}", path.display());
    }
  }

  /// Adds the files of a video made before everything went to the library: every picture, clip
  /// and track in its folder that isn't `known` to be there already, old versions included.
  fn catalogue(&self, dir: &Path, known: &BTreeSet<PathBuf>) {
    let files = project_media(dir);
    let missing: Vec<&(PathBuf, MediaClass)> = files.iter().filter(|(path, _)| !known.contains(path)).collect();
    let mut added = Vec::new();
    for (n, (path, class)) in missing.iter().enumerate() {
      if self.stopped() {
        break;
      }
      self.send(SongUpdate::Note(format!("Adding this video's files to the library\u{2026} {} of {}", n + 1, missing.len())));
      match std::fs::read(path) {
        Ok(bytes) => {
          self.add_to_library(path, &bytes, *class);
          added.push(path.clone());
        },
        Err(err) => warn!("Couldn't read {}: {err}", path.display()),
      }
    }
    self.send(SongUpdate::Catalogued(added));
  }

  fn charge(&self, account: Account, usd: f64, item: &str) {
    self.engine.charge_for(self.tx, vec![(account, usd, item.to_owned())], &self.batch, self.project.as_deref().map(|project| (project, self.part)));
  }

  /// Runs a provider call at the run's pace, waiting and trying again when its quota is met and
  /// saying so on the page.
  async fn patiently<T, F, Fut>(&self, call: F) -> anyhow::Result<T>
  where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
  {
    let waiting = |wait: Duration| self.send(SongUpdate::Note(format!("Google is busy or at its limit; waiting {}s and going slower\u{2026}", wait.as_secs())));
    with_quota_retries(&self.pace, || !self.stopped(), waiting, call).await
  }

  fn stopped(&self) -> bool {
    self.cancel.load(Ordering::Relaxed)
  }

  fn send(&self, update: SongUpdate) {
    let _ = self.tx.send(Event::Song { run_id: self.run_id, update });
    self.ctx.request_repaint();
  }
}

/// How the lyrics are put to the music model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wording {
  /// Directions taken out of the lyrics and given as arrangement notes.
  Sorted,
  /// Directions taken out and left out.
  WithoutNotes,
  /// The lyrics exactly as typed.
  AsTyped,
}

impl Wording {
  /// What the page says when a refusal leads to this wording.
  fn note(self) -> Option<&'static str> {
    match self {
      Wording::Sorted => Some("Google's filter refused the song; asking again\u{2026}"),
      Wording::WithoutNotes => Some("Refused again; trying without the directions written in the lyrics\u{2026}"),
      Wording::AsTyped => Some("Refused again; trying the lyrics exactly as typed (bracketed directions may be sung)\u{2026}"),
    }
  }
}

/// Which of the user's Google accounts paid for a file: the Cloud project, or the Gemini API
/// key's prepaid balance when Vertex had no capacity and the key was used.
fn google_account(file: &GeneratedFile) -> Account {
  if file.via_key { Account::GeminiApi } else { Account::Vertex }
}

/// Whether a provider refused the prompt itself (nothing was made or billed).
fn is_blocked(err: &anyhow::Error) -> bool {
  let message = format!("{err:#}").to_lowercase();
  ["input blocked", "prohibited use", "sensitive words"].iter().any(|sign| message.contains(sign))
}

/// What Lyria is given: the direction first, then the lyrics under their own heading.
fn music_prompt(draft: &Draft, length: SongLength, instrumental: bool, wording: Wording) -> String {
  // The direction is named as the thing to follow: sent bare above a page of lyrics, it reads as
  // one more line and the words end up choosing the genre. The as-typed wording is the plain
  // one that is fallen back to when a prompt is refused.
  let mut prompt = match (wording, draft.style.trim()) {
    (Wording::AsTyped, style) => format!("{style}\n{}", length.music_line()),
    (_, "") => length.music_line().to_owned(),
    (_, style) => format!("Musical style, to follow closely for the genre, tempo, instruments, vocal and production, whatever the lyrics suggest:\n{style}\n\n{}", length.music_line()),
  };
  if instrumental || draft.lyrics.trim().is_empty() {
    prompt.push_str("\nInstrumental only, no vocals.");
  } else {
    // Notes to the band that were written among the lyrics go with the direction, where they
    // can't be mistaken for words; the lyrics are left as bare section tags and sung lines.
    if wording == Wording::AsTyped {
      prompt.push_str(&format!("\n\nSing these lyrics as written, following their section tags:\n\n{}", draft.lyrics.trim()));
      return prompt;
    }
    let tidied = plan::tidy_lyrics(&draft.lyrics);
    if !tidied.directions.is_empty() && wording == Wording::Sorted {
      prompt.push_str(&format!("\nArrangement notes, for the instruments and production only, never sung or spoken: {}.", tidied.directions.join("; ")));
    }
    prompt.push_str(&format!("\n\nLyrics, to sing as written in the style above. A line in square brackets is a section label only: follow it, and never sing, speak or read it out.\n\n{}", tidied.lyrics));
  }
  prompt
}

fn reference_prompt(kind: ReferenceKind, name: &str, description: &str, look: &str) -> String {
  let subject = match kind {
    ReferenceKind::Look => format!("A character reference sheet for a music video, on a plain white background. On the left, a large head-and-shoulders portrait of the performer facing the camera, the face evenly lit and sharp. On the right, the same performer shown full-length three times side by side, from the front, the side and the back, the whole figure in frame from head to shoes each time. It is one person throughout, with the same face, hair and clothes in every view. The performer, {}: {}", name.trim(), sentence(description)),
    ReferenceKind::Face => format!("A head-and-shoulders portrait of a music video's performer facing the camera, on a plain white background, the face filling most of the frame, evenly lit and sharp. The performer, {}: {}", name.trim(), sentence(description)),
    ReferenceKind::Location => format!("An establishing shot of a music video's location with nobody in it. The place, {}: {}", name.trim(), sentence(description)),
    ReferenceKind::Prop => format!("A reference photograph of a single prop from a music video, alone on a plain white background. The prop, {}: {}", name.trim(), sentence(description)),
  };
  format!("{subject} The video's look, which this picture must match: {}{} No text, captions or logos.", sentence(look), no_colour(look))
}

/// Whether the video's look is black and white.
fn is_monochrome(look: &str) -> bool {
  let look = look.to_lowercase();
  ["black and white", "black-and-white", "black & white", "b&w", "monochrome", "greyscale", "grayscale"].iter().any(|word| look.contains(word))
}

/// The line a black-and-white look adds to a prompt: image models slip colour in otherwise.
fn no_colour(look: &str) -> &'static str {
  if is_monochrome(look) { " Strictly black and white: no colour anywhere in the frame." } else { "" }
}

/// A picture as the video's look needs it: a black-and-white look is not left to the model, so
/// one that comes back with colour in it has the colour taken out.
fn in_keeping(file: GeneratedFile, look: &str) -> GeneratedFile {
  if !is_monochrome(look) {
    return file;
  }
  let grey = tokio::task::block_in_place(|| -> anyhow::Result<Vec<u8>> {
    let image = image::load_from_memory(&file.bytes)?;
    let mut out = Vec::new();
    image.grayscale().write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
  });
  match grey {
    Ok(bytes) => GeneratedFile { bytes, extension: "png", usage: file.usage, via_key: file.via_key },
    Err(err) => {
      warn!("Couldn't take the colour out of a picture: {err:#}");
      file
    },
  }
}

/// The performer's head shot, drawn from their sheet so it is the same person.
fn face_from_sheet_prompt(look: &str) -> String {
  format!("The attached picture is a performer's reference sheet for a music video. Draw a head-and-shoulders portrait of this exact person: the same face, hair, skin, age and clothes as on the sheet, changing nothing about them. They face the camera with a neutral expression, the face filling most of the frame, evenly lit and sharp, on a plain white background. One picture of one person; don't copy the sheet's layout. The video's look, which this picture must match: {}{} No text, captions or logos.", sentence(look), no_colour(look))
}

/// The performer's sheet, drawn from their head shot so it is the same person. The head shot
/// settles the face; the description settles what it doesn't show.
fn sheet_from_face_prompt(name: &str, description: &str, look: &str) -> String {
  format!(
    "The attached picture is a head shot of a music video's performer. Draw a character reference sheet of this exact person, on a plain white background: the same face, hair, skin and age as in the head shot, changing nothing about them. \
     On the left, a large head-and-shoulders portrait facing the camera, the face evenly lit and sharp. On the right, the same person shown full-length three times side by side, from the front, the side and the back, the whole figure in frame from head to shoes each time. \
     It is one person throughout, with the same face, hair and clothes in every view. For what the head shot doesn't show, their build, clothes and shoes, follow this description of the performer, {}, and ignore anything in it about the face or hair that differs from the head shot: {} \
     The video's look, which this picture must match: {}{} No text, captions or logos.",
    name.trim(),
    sentence(description),
    sentence(look),
    no_colour(look)
  )
}

/// A written change to a reference picture the user already has.
fn change_prompt(change: &str) -> String {
  format!("Change the attached reference picture as follows: {} Keep everything else exactly as it is: the same subject, pose, framing, background and style. No text, captions or logos.", sentence(change))
}

/// A still's prompt. The attached pictures are named one by one, in the order they are sent
/// (`kinds`, after the draft frame when `guided`), because which of them exist varies: calling the
/// location "the performer's reference sheet" when the sheet is missing draws the wrong person.
fn still_prompt(design: &Design, image: &str, kinds: &[ReferenceKind], guided: bool, ends_shot: bool, sing: bool) -> String {
  let mut parts = vec![format!("A single frame from a music video. {}", sentence(image))];
  // The mouth is redrawn afterwards to the vocal, which fails when anything sits in front of it.
  if sing {
    parts.push(SUNG_FRAME.to_owned());
  }
  let pictures: Vec<&str> = guided.then_some("the approved draft of this exact frame").into_iter().chain(ends_shot.then_some("the frame this same shot opens on")).chain(kinds.iter().map(|kind| match kind {
    ReferenceKind::Look => "the performer's reference sheet",
    ReferenceKind::Face => "the performer's face in close-up",
    ReferenceKind::Location => "the location",
    ReferenceKind::Prop => "a prop",
  })).collect();
  match pictures.as_slice() {
    [] => {},
    [only] => parts.push(format!("The attached picture is {only}.")),
    many => parts.push(format!("The attached pictures, in order: {}.", many.iter().enumerate().map(|(i, what)| format!("{}, {what}", i + 1)).collect::<Vec<_>>().join("; "))),
  }
  // An ending frame is the same shot a few seconds on: it has to cut together with its opening.
  if ends_shot {
    parts.push("This is the frame the shot ends on, a few seconds after its opening frame. It is the same place, the same light, and the performer in the same clothes with the same props, as the opening frame shows them; only the framing and the action are as described here.".to_owned());
  }
  // The final pass: the director has approved this frame, so it's recreated, not reimagined.
  if guided {
    parts.push("Recreate the approved draft faithfully: the same composition, framing, pose, expression and lighting, with finer detail and cleaner rendering. Change nothing about what is in the frame or where.".to_owned());
  }
  let (sheet, face) = (kinds.contains(&ReferenceKind::Look), kinds.contains(&ReferenceKind::Face));
  // The scene's own words about the performer, and a draft drawn by a weaker model, both drift.
  let whatever = format!("whatever this scene's wording{} suggests", if guided { " or the draft" } else { "" });
  match (sheet, face) {
    (true, _) => {
      let close_up = if face { " Take the face from the close-up, which shows it in full detail: the same features exactly." } else { "" };
      parts.push(format!("Wherever the performer appears, they are the person on the reference sheet: the same face, hair, build and clothes, {whatever}.{close_up} Show them once, as part of the scene; don't copy the sheet's layout or its plain background. For the record, they are: {}", sentence(&design.character)));
    },
    (false, true) => parts.push(format!("Wherever the performer appears, they are the person in the close-up: the same face and hair exactly, {whatever}. Don't copy its plain background. For the record, they are: {}", sentence(&design.character))),
    (false, false) => parts.push(format!("The performer: {}", sentence(&design.character))),
  }
  if kinds.iter().any(|kind| matches!(kind, ReferenceKind::Location | ReferenceKind::Prop)) {
    parts.push("Keep the location and the props as their pictures show them.".to_owned());
  }
  if !design.venue.trim().is_empty() {
    parts.push(format!("The venue: {}", sentence(&design.venue)));
  }
  parts.push(format!("The video's look, which this frame must match: {}{} No text, captions, subtitles, borders or logos.", sentence(&design.look), no_colour(&design.look)));
  parts.join(" ")
}

fn clip_prompt(look: &str, motion: &str, sing: bool, ends: bool) -> String {
  // A sung scene's mouth is redrawn afterwards; that works when the face stays in view and the mouth is already moving.
  let singing = if sing { " The performer sings to the camera throughout, mouth moving and clearly visible, face towards the lens, head fairly steady. Nothing comes between the camera and the mouth at any moment: no microphone (the performer holds none, and no mic or stand is in front of the face), no hands, hair, cup, smoke, prop or passer-by, and the performer never turns away, looks down or covers the face. The lower face stays lit and in focus for the whole shot." } else { "" };
  let ending = if ends { " The shot moves in one continuous take from the first frame to the last frame given, and ends exactly on the last frame." } else { "" };
  format!("{}{singing}{ending} Keep the look of the opening frame: {}{} No text or captions appear.", sentence(motion), sentence(look), no_colour(look))
}

/// `text` trimmed, ending in a full stop.
fn sentence(text: &str) -> String {
  let text = text.trim();
  if text.is_empty() || text.ends_with(['.', '!', '?']) {
    text.to_owned()
  } else {
    format!("{text}.")
  }
}

fn still_model(pro: bool) -> &'static str {
  if pro {
    NANO_BANANA_PRO
  } else {
    NANO_BANANA
  }
}

/// Every picture, clip and track in a video's folder, in a steady order: the folder itself and
/// its `references`, `shots` and `clips`. The grid's small copies and the project file are not media.
fn project_media(dir: &Path) -> Vec<(PathBuf, MediaClass)> {
  let mut out = Vec::new();
  for folder in [dir.to_path_buf(), dir.join("references"), dir.join("shots"), dir.join("clips")] {
    let mut files: Vec<PathBuf> = std::fs::read_dir(&folder).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| path.is_file()).collect();
    files.sort();
    for path in files {
      let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
      if name.starts_with('.') || name.ends_with(".thumb.jpg") {
        continue;
      }
      let class = match name.rsplit('.').next().unwrap_or_default() {
        "png" | "jpg" | "jpeg" | "webp" => MediaClass::Image,
        "mp4" => MediaClass::Video,
        "mp3" | "wav" | "m4a" | "aac" | "flac" | "ogg" | "opus" => MediaClass::Audio,
        _ => continue,
      };
      out.push((path, class));
    }
  }
  out
}

/// A library title for one of a video's files: the video's name, then what the file is, read
/// from its name (`shot_012_final_v3.png` is "scene 12 still, final, v3").
fn asset_title(path: &Path) -> String {
  let video = path.ancestors().skip(1).take(2).find_map(|dir| std::fs::read_to_string(dir.join("project.json")).ok()).and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|project| project.get("title").and_then(Value::as_str).map(str::trim).filter(|title| !title.is_empty()).map(str::to_owned)).unwrap_or_else(|| "Music video".to_owned());
  format!("{video} \u{b7} {}", describe_asset(&path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default()))
}

fn describe_asset(stem: &str) -> String {
  let mut words = stem.split('_');
  let what = match words.next().unwrap_or_default() {
    "shot" => format!("scene {} still", number(words.next())),
    "clip" => format!("scene {} clip", number(words.next())),
    "look" => "performer".to_owned(),
    "face" => "face".to_owned(),
    "location" => "location".to_owned(),
    "prop" => "prop".to_owned(),
    "song" => "song".to_owned(),
    "vocals" => "vocal stem".to_owned(),
    "cut" => "the film".to_owned(),
    other => other.to_owned(),
  };
  let notes: Vec<String> = words.filter_map(|word| match word {
    "draft" | "final" => Some(word.to_owned()),
    "synced" => Some("lip-synced".to_owned()),
    "instagram" => Some("for Instagram".to_owned()),
    "imported" => Some("imported".to_owned()),
    version if version.starts_with('v') && version[1..].chars().all(|c| c.is_ascii_digit()) => Some(version.to_owned()),
    _ => None,
  }).collect();
  if notes.is_empty() { what } else { format!("{what}, {}", notes.join(", ")) }
}

/// "012" as "12"; anything else as it is.
fn number(word: Option<&str>) -> String {
  let word = word.unwrap_or_default();
  word.parse::<u32>().map_or_else(|_| word.to_owned(), |n| n.to_string())
}

/// Writes `bytes` to `dir/name`, making the folder.
fn save(dir: &Path, name: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
  std::fs::create_dir_all(dir)?;
  let path = dir.join(name);
  std::fs::write(&path, bytes)?;
  Ok(path)
}

/// Saves a picture and a small copy beside it ([`thumb_path`]) for the page's grids, so a
/// storyboard of dozens of stills doesn't hold every full-size image in memory.
fn save_picture(dir: &Path, name: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
  let path = save(dir, name, bytes)?;
  let thumb = tokio::task::block_in_place(|| -> anyhow::Result<()> {
    let image = image::load_from_memory(bytes)?;
    let small = image.resize(THUMB_WIDTH, THUMB_WIDTH, image::imageops::FilterType::Triangle).to_rgb8();
    small.save_with_format(thumb_path(&path), image::ImageFormat::Jpeg)?;
    Ok(())
  });
  if let Err(err) = thumb {
    warn!("Couldn't make a small copy of {}: {err:#}", path.display());
  }
  Ok(path)
}

/// Where the small copy of the picture at `path` is kept.
pub fn thumb_path(path: &Path) -> PathBuf {
  path.with_extension("thumb.jpg")
}

/// A run's shared pace. Dozens of stills and clips in a row meet a provider's per-minute limit
/// sooner or later; when one call is turned away, every call of the run holds off and they start
/// further apart from then on, instead of each retrying on its own and colliding again.
struct Pace {
  first_wait: Duration,
  state: tokio::sync::Mutex<PaceState>,
}

struct PaceState {
  /// When the next call may start.
  next_start: tokio::time::Instant,
  /// How far apart calls start (zero until the first refusal).
  gap: Duration,
}

impl Pace {
  fn new(first_wait: Duration) -> Self {
    Self { first_wait, state: tokio::sync::Mutex::new(PaceState { next_start: tokio::time::Instant::now(), gap: Duration::ZERO }) }
  }

  /// Waits for this call's turn; `false` when the run was stopped while waiting.
  async fn turn(&self, keep_going: &impl Fn() -> bool) -> bool {
    let start = {
      let mut state = self.state.lock().await;
      let start = state.next_start.max(tokio::time::Instant::now());
      state.next_start = start + state.gap;
      start
    };
    while tokio::time::Instant::now() < start {
      if !keep_going() {
        return false;
      }
      tokio::time::sleep(STOP_CHECK.min(start - tokio::time::Instant::now())).await;
    }
    keep_going()
  }

  /// A call was turned away: nothing starts for `wait`, and calls start further apart after it.
  async fn slow_down(&self, wait: Duration) {
    let mut state = self.state.lock().await;
    // No gap when there is no wait either (tests).
    if !wait.is_zero() {
      state.gap = (state.gap * 2).clamp(QUOTA_FIRST_GAP, QUOTA_MAX_GAP);
    }
    state.next_start = state.next_start.max(tokio::time::Instant::now() + wait);
  }
}

/// Runs `call` at `pace`, and when the provider turns it away for quota slows the pace
/// (`waiting` is told for how long) and tries again, up to [`QUOTA_RETRIES`] times.
async fn with_quota_retries<T, F, Fut>(pace: &Pace, keep_going: impl Fn() -> bool, waiting: impl Fn(Duration), mut call: F) -> anyhow::Result<T>
where
  F: FnMut() -> Fut,
  Fut: Future<Output = anyhow::Result<T>>,
{
  let mut wait = pace.first_wait;
  for attempt in 0..=QUOTA_RETRIES {
    anyhow::ensure!(pace.turn(&keep_going).await, "Stopped");
    match call().await {
      Err(err) if is_quota(&err) && attempt < QUOTA_RETRIES => {
        info!("Provider quota reached; holding the run for {}s", wait.as_secs());
        waiting(wait);
        pace.slow_down(wait).await;
        wait = (wait * 2).min(QUOTA_MAX_WAIT);
      },
      Err(err) if is_quota(&err) => return Err(err.context("Google was still busy or at its limit after several minutes of waiting. Give it a few minutes, then use the same button: it makes only the ones still missing")),
      result => return result,
    }
  }
  unreachable!("the last attempt returns")
}

/// Whether the provider turned the call away for now, not for good: its quota is met, or (Veo's
/// "experiencing high load", a 503) it has no room at the moment.
fn is_quota(err: &anyhow::Error) -> bool {
  let text = format!("{err:#}");
  let lower = text.to_ascii_lowercase();
  text.contains("error 429") || text.contains("error 503") || text.contains("RESOURCE_EXHAUSTED") || text.contains("UNAVAILABLE") || ["quota", "high load", "try again later", "overloaded"].iter().any(|word| lower.contains(word))
}

#[cfg(test)]
mod tests {
  use std::io::{BufRead, BufReader, Read, Write};
  use std::net::TcpListener;
  use std::sync::Mutex;
  use std::sync::mpsc::Receiver;

  use base64::Engine;
  use serde_json::{Value, json};

  use super::*;
  use crate::backend::local::config::LocalConfig;
  use crate::backend::local::song::plan::Energy;

  mod runs {
    use super::*;

    /// Writing a song goes to Gemini with the songwriter's brief, is charged, and comes back as a
    /// draft.
    #[tokio::test(flavor = "multi_thread")]
    async fn writing_a_song_asks_gemini_and_charges_for_it() {
      let draft = json!({ "title": "Night Bus", "style": "slow, 90 BPM", "lyrics": "[Verse 1]\nline one\n[Chorus]\nline two" });
      let (base_url, requests) = fake_vertex(vec![text_reply(&draft.to_string(), 1000, 400)]);
      let (engine, dir) = engine(base_url);
      let (tx, rx) = std::sync::mpsc::channel();
      engine.start_song(7, SongRequest::Write { idea: "the last bus home".into(), notes: String::new(), current: None, length: SongLength::TwoMinutes, writer: Writer::Gemini }, tx, egui::Context::default());

      let updates = finished(&rx, 7).await;
      assert!(matches!(&updates[0], SongUpdate::Written(d) if d.title == "Night Bus" && d.lyrics.contains("[Chorus]")), "{updates:?}");
      assert!(matches!(updates.last(), Some(SongUpdate::Done(Ok(())))));
      let requests = requests.lock().unwrap();
      assert_eq!(requests[0].0, "POST /v1beta1/publishers/google/models/gemini-3.8-flash:generateContent HTTP/1.1");
      let body: Value = serde_json::from_str(&requests[0].1).unwrap();
      assert!(body["systemInstruction"]["parts"][0]["text"].as_str().unwrap().contains("wholly original"));
      assert!(body["contents"][0]["parts"][0]["text"].as_str().unwrap().contains("the last bus home"));
      // 1,000 tokens in and 400 out at the Flash rates.
      assert!((engine.spend_summary().today.vertex - (1000.0 * 0.50 + 400.0 * 3.00) / 1e6).abs() < 1e-9);
      let _ = std::fs::remove_dir_all(dir);
    }

    /// A storyboard has no holes even when the writer skips shots.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_plan_fills_the_shots_the_writer_left_out() {
      let reply = json!({ "scenes": [{ "index": 1, "image": "A close-up of hands on a keyboard", "motion": "Fingers hammer the keys.", "beat": "Frantic" }] });
      let (base_url, _requests) = fake_vertex(vec![text_reply(&reply.to_string(), 10, 10)]);
      let (engine, dir) = engine(base_url);
      let slots = vec![slot(0, Energy::Low), slot(1, Energy::High)];
      let (tx, rx) = std::sync::mpsc::channel();
      engine.start_song(1, SongRequest::Plan { brief: Brief::default(), style: Style::default(), slots, lip_sync: false, writer: Writer::Gemini }, tx, egui::Context::default());

      let updates = finished(&rx, 1).await;
      let planned = updates.iter().find_map(|u| if let SongUpdate::Planned(shots) = u { Some(shots) } else { None }).unwrap();
      assert_eq!(planned.len(), 2);
      assert!(planned[0].image.contains("wide shot"), "the skipped low-energy slot gets a stand-in");
      assert_eq!(planned[1].image, "A close-up of hands on a keyboard");
      assert!(planned.iter().all(|scene| !scene.ending.is_empty()), "every scene ends somewhere, written or not: {planned:?}");
      assert!(updates.iter().any(|u| matches!(u, SongUpdate::Note(n) if n.contains("left out 1 of 2"))));
      let _ = std::fs::remove_dir_all(dir);
    }

    /// Scenes planned without ending frames are given them: the writer's where it answers, the
    /// scene's own movement where it doesn't.
    #[tokio::test(flavor = "multi_thread")]
    async fn missing_ending_frames_are_written() {
      let reply = json!({ "scenes": [{ "index": 4, "ending": "The dock fills the frame" }] });
      let (base_url, requests) = fake_vertex(vec![text_reply(&reply.to_string(), 10, 10)]);
      let (engine, dir) = engine(base_url);
      let scenes = vec![(4, "Verse 1".to_owned(), 4.0, "A dock, far off.".to_owned(), "The camera pushes in.".to_owned()), (7, "Chorus".to_owned(), 6.0, "A hull.".to_owned(), "The performer turns.".to_owned())];
      let (tx, rx) = std::sync::mpsc::channel();
      engine.start_song(11, SongRequest::Endings { brief: Brief::default(), scenes, writer: Writer::Gemini }, tx, egui::Context::default());
      let updates = finished(&rx, 11).await;
      let endings = updates.iter().find_map(|u| if let SongUpdate::Endings(endings) = u { Some(endings.clone()) } else { None }).unwrap();
      assert_eq!(endings[0], (4, "The dock fills the frame".to_owned()));
      assert!(endings[1].0 == 7 && endings[1].1.contains("The performer turns"), "{endings:?}");
      assert_eq!(requests.lock().unwrap().len(), 1);
      let _ = std::fs::remove_dir_all(dir);
    }

    /// Reference images are drawn in the style's look, saved in the project and added to the
    /// library; the performer's is a three-view sheet.
    #[tokio::test(flavor = "multi_thread")]
    async fn reference_images_are_made_for_the_performer_and_the_place() {
      let (base_url, requests) = fake_vertex(vec![image_reply(), image_reply()]);
      let (engine, dir) = engine(base_url);
      let project = dir.join("project");
      let items = vec![ReferenceJob { index: 0, version: 1, kind: ReferenceKind::Look, name: "The Orchestrator".into(), description: "Wiry, cable hair".into(), change: None, sheet: None }, ReferenceJob { index: 1, version: 3, kind: ReferenceKind::Location, name: "Rail Vault".into(), description: "A brick tunnel".into(), change: None, sheet: None }];
      let (tx, rx) = std::sync::mpsc::channel();
      engine.start_song(4, SongRequest::References { dir: project.clone(), look: "Raw 16mm black-and-white film".into(), aspect: Aspect::Wide, pro: false, items }, tx, egui::Context::default());

      let updates = finished(&rx, 4).await;
      let mut made: Vec<(usize, &PathBuf)> = updates.iter().filter_map(|u| if let SongUpdate::Reference { index, path } = u { Some((*index, path)) } else { None }).collect();
      made.sort();
      assert_eq!(made.len(), 2);
      assert!(made[0].1.ends_with("references/look_01_v1.png") && made[1].1.ends_with("references/location_02_v3.png"));
      assert_eq!(engine.library().list("image", false, 0, 40).0.len(), 2);
      let requests = requests.lock().unwrap();
      let prompts: Vec<String> = requests.iter().map(|(_, body)| serde_json::from_str::<Value>(body).unwrap()["contents"][0]["parts"][0]["text"].as_str().unwrap().to_owned()).collect();
      assert!(prompts.iter().any(|p| p.contains("from the front, the side and the back") && p.contains("Wiry, cable hair.") && p.contains("Raw 16mm black-and-white film.")));
      assert!(prompts.iter().any(|p| p.contains("with nobody in it") && p.contains("Rail Vault")));
      let _ = std::fs::remove_dir_all(dir);
    }

    /// An uploaded picture is copied into the project as a PNG with its small copy, at no charge;
    /// a written change sends the current picture back with the change.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_reference_can_be_uploaded_and_then_changed() {
      let (base_url, requests) = fake_vertex(vec![image_reply()]);
      let (engine, dir) = engine(base_url);
      let project = dir.join("project");
      std::fs::create_dir_all(&dir).unwrap();
      let mine = dir.join("mine.jpg");
      image::RgbImage::from_pixel(8, 8, image::Rgb([10, 20, 30])).save(&mine).unwrap();
      let (tx, rx) = std::sync::mpsc::channel();
      engine.start_song(5, SongRequest::ImportReference { dir: project.clone(), index: 0, version: 2, kind: ReferenceKind::Look, file: mine }, tx.clone(), egui::Context::default());
      let updates = finished(&rx, 5).await;
      let Some(SongUpdate::Reference { index: 0, path }) = updates.first() else { panic!("{updates:?}") };
      assert!(path.ends_with("references/look_01_v2.png") && thumb_path(path).exists());
      assert!(image::open(path).unwrap().to_rgb8().get_pixel(0, 0).0[2].abs_diff(30) < 6, "the picture itself is kept");
      assert!(requests.lock().unwrap().is_empty(), "nothing is generated for an upload");

      let item = ReferenceJob { index: 0, version: 3, kind: ReferenceKind::Look, name: "The Captain".into(), description: "Wiry".into(), change: Some((path.clone(), "a longer coat".into())), sheet: None };
      engine.start_song(6, SongRequest::References { dir: project, look: "grainy".into(), aspect: Aspect::Wide, pro: false, items: vec![item] }, tx, egui::Context::default());
      let updates = finished(&rx, 6).await;
      assert!(matches!(updates.first(), Some(SongUpdate::Reference { index: 0, path }) if path.ends_with("references/look_01_v3.png")), "{updates:?}");
      let requests = requests.lock().unwrap();
      let parts = serde_json::from_str::<Value>(&requests[0].1).unwrap()["contents"][0]["parts"].clone();
      assert_eq!(parts[0]["inlineData"]["mimeType"], "image/png");
      assert!(parts[1]["text"].as_str().unwrap().starts_with("Change the attached reference picture as follows: a longer coat. Keep everything else"), "{parts}");
      let _ = std::fs::remove_dir_all(dir);
    }

    /// The head shot is drawn from the sheet, so it is the same person: the sheet goes with the
    /// request, square, and the description isn't what it is drawn from.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_head_shot_is_drawn_from_the_sheet() {
      let (base_url, requests) = fake_vertex(vec![image_reply()]);
      let (engine, dir) = engine(base_url);
      let project = dir.join("project");
      let sheet = save(&project.join("references"), "look_01_v1.png", &tiny_png()).unwrap();
      let (tx, rx) = std::sync::mpsc::channel();
      let items = vec![ReferenceJob { index: 1, version: 1, kind: ReferenceKind::Face, name: "The Captain".into(), description: "Wiry, cable hair".into(), change: None, sheet: Some(sheet) }];
      engine.start_song(13, SongRequest::References { dir: project, look: "grainy".into(), aspect: Aspect::Wide, pro: false, items }, tx, egui::Context::default());
      let updates = finished(&rx, 13).await;
      assert!(matches!(updates.first(), Some(SongUpdate::Reference { index: 1, path }) if path.ends_with("references/face_02_v1.png")), "{updates:?}");
      let requests = requests.lock().unwrap();
      let body: Value = serde_json::from_str(&requests[0].1).unwrap();
      assert!(body["contents"][0]["parts"][0]["inlineData"]["data"].is_string(), "the sheet is attached");
      let prompt = body["contents"][0]["parts"][1]["text"].as_str().unwrap();
      assert!(prompt.contains("Draw a head-and-shoulders portrait of this exact person") && !prompt.contains("Wiry, cable hair"), "{prompt}");
      assert_eq!(body["generationConfig"]["imageConfig"]["aspectRatio"], "1:1");
      let _ = std::fs::remove_dir_all(dir);
    }

    /// The other way round: with a head shot given, the three-pose sheet is drawn of that person,
    /// taking the build and clothes from the description.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_sheet_is_drawn_from_a_head_shot() {
      let (base_url, requests) = fake_vertex(vec![image_reply()]);
      let (engine, dir) = engine(base_url);
      let project = dir.join("project");
      let face = save(&project.join("references"), "face_02_v1.png", &tiny_png()).unwrap();
      let (tx, rx) = std::sync::mpsc::channel();
      let items = vec![ReferenceJob { index: 0, version: 1, kind: ReferenceKind::Look, name: "The Captain".into(), description: "Wiry, in a long coat".into(), change: None, sheet: Some(face) }];
      engine.start_song(14, SongRequest::References { dir: project, look: "grainy".into(), aspect: Aspect::Wide, pro: false, items }, tx, egui::Context::default());
      let updates = finished(&rx, 14).await;
      assert!(matches!(updates.first(), Some(SongUpdate::Reference { index: 0, path }) if path.ends_with("references/look_01_v1.png")), "{updates:?}");
      let requests = requests.lock().unwrap();
      let body: Value = serde_json::from_str(&requests[0].1).unwrap();
      assert!(body["contents"][0]["parts"][0]["inlineData"]["data"].is_string(), "the head shot is attached");
      let prompt = body["contents"][0]["parts"][1]["text"].as_str().unwrap();
      assert!(prompt.contains("Draw a character reference sheet of this exact person") && prompt.contains("from the front, the side and the back") && prompt.contains("Wiry, in a long coat."), "{prompt}");
      let _ = std::fs::remove_dir_all(dir);
    }

    /// Stills are saved in the project's folder with the performer's picture sent as a reference;
    /// one that fails is reported without stopping the others.
    #[tokio::test(flavor = "multi_thread")]
    async fn stills_land_in_the_project_and_failures_dont_stop_the_rest() {
      let blocked = json!({ "candidates": [{ "finishReason": "IMAGE_SAFETY" }] }).to_string();
      let (base_url, requests) = fake_vertex(vec![image_reply(), blocked]);
      let (engine, dir) = engine(base_url);
      let project = dir.join("project");
      let portrait = save(&project, "portrait_v1.png", &tiny_png()).unwrap();
      let shots = vec![StillJob { index: 0, version: 1, image: "The performer under a strip light".into(), guide: None, ending: false, opening: None, after: None, sing: false }, StillJob { index: 1, version: 2, image: "A tunnel".into(), guide: None, ending: false, opening: None, after: None, sing: false }];
      let (tx, rx) = std::sync::mpsc::channel();
      engine.start_song(3, SongRequest::Stills { dir: project.clone(), design: Design { look: "black and white".into(), character: "a dancer".into(), venue: "a tunnel".into() }, references: vec![(ReferenceKind::Look, portrait)], aspect: Aspect::Tall, quality: Quality::Draft, better: false, shots }, tx, egui::Context::default());

      let updates = finished(&rx, 3).await;
      let made: Vec<&PathBuf> = updates.iter().filter_map(|u| if let SongUpdate::Still { path, .. } = u { Some(path) } else { None }).collect();
      assert_eq!(made.len(), 1);
      assert!(made[0].starts_with(project.join("shots")) && made[0].exists());
      assert!(image::open(thumb_path(made[0])).is_ok(), "a small copy is kept for the grid");
      assert!(updates.iter().any(|u| matches!(u, SongUpdate::ShotFailed { message, .. } if message.contains("IMAGE_SAFETY"))));
      assert!(matches!(updates.last(), Some(SongUpdate::Done(Ok(())))), "a failed shot doesn't fail the run");
      let requests = requests.lock().unwrap();
      let body: Value = serde_json::from_str(&requests[0].1).unwrap();
      assert!(body["contents"][0]["parts"][0]["inlineData"]["data"].is_string(), "the portrait goes first");
      assert_eq!(body["generationConfig"]["imageConfig"], json!({ "aspectRatio": "9:16", "imageSize": "1K" }));
      let prompt = body["contents"][0]["parts"][1]["text"].as_str().unwrap();
      assert!(prompt.contains("they are the person on the reference sheet") && prompt.contains("The venue: a tunnel."));
      let in_library = engine.library().list("image", false, 0, 40).0;
      assert_eq!(in_library.iter().map(|file| file.maybe_title.as_deref()).collect::<Vec<_>>(), [Some("Music video \u{b7} scene 1 still, draft, v1")], "every still that is made goes in the library, titled");
      let _ = std::fs::remove_dir_all(dir);
    }

    /// Lip sync needs fal: without a key the run says how to connect it, and calls nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn lip_sync_without_a_fal_key_says_how_to_connect() {
      let (engine, dir) = engine("http://127.0.0.1:9/".to_owned());
      let (tx, rx) = std::sync::mpsc::channel();
      let shots = vec![SyncJob { index: 0, version: 1, clip: dir.join("c.mp4"), start: 0.0, end: 2.0 }];
      engine.start_song(5, SongRequest::LipSync { dir: dir.join("p"), track: dir.join("song.mp3"), vocals: Some(dir.join("v.mp3")), quality: Quality::Draft, shots }, tx, egui::Context::default());
      let updates = finished(&rx, 5).await;
      assert!(matches!(updates.last(), Some(SongUpdate::Done(Err(message))) if message.contains("fal API key in Settings")), "{updates:?}");
      assert_eq!(engine.spend_summary().today.fal, 0.0);
      let _ = std::fs::remove_dir_all(dir);
    }

    /// An ending frame is drawn with its scene's opening frame beside it, ahead of the references,
    /// and saved and reported as the scene's ending.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_ending_frame_is_drawn_to_match_its_opening_frame() {
      let (base_url, requests) = fake_vertex(vec![image_reply()]);
      let (engine, dir) = engine(base_url);
      let project = dir.join("p");
      let opening = save(&project.join("shots"), "shot_001_draft_v1.png", &tiny_png()).unwrap();
      let sheet = save(&project.join("references"), "look_01_v1.png", &tiny_png()).unwrap();
      let (tx, rx) = std::sync::mpsc::channel();
      let shots = vec![StillJob { index: 0, version: 2, image: "The performer's face fills the frame".into(), guide: None, ending: true, opening: Some(opening), after: None, sing: false }];
      engine.start_song(16, SongRequest::Stills { dir: project, design: Design { look: "grainy".into(), character: "a dancer".into(), venue: String::new() }, references: vec![(ReferenceKind::Look, sheet)], aspect: Aspect::Wide, quality: Quality::Draft, better: false, shots }, tx, egui::Context::default());
      let updates = finished(&rx, 16).await;
      assert!(matches!(updates.first(), Some(SongUpdate::Still { index: 0, path, ending: true }) if path.ends_with("shots/shot_001_draft_end_v2.png")), "{updates:?}");
      let requests = requests.lock().unwrap();
      let parts = serde_json::from_str::<Value>(&requests[0].1).unwrap()["contents"][0]["parts"].clone();
      assert!(parts[0]["inlineData"]["data"].is_string() && parts[1]["inlineData"]["data"].is_string(), "the opening frame, then the sheet");
      let prompt = parts[2]["text"].as_str().unwrap();
      assert!(prompt.contains("in order: 1, the frame this same shot opens on; 2, the performer's reference sheet."), "{prompt}");
      assert!(prompt.contains("This is the frame the shot ends on") && prompt.contains("The performer's face fills the frame."));
      let _ = std::fs::remove_dir_all(dir);
    }

    /// A run that draws a scene's opening and ending draws the opening first and matches the
    /// ending to it; an ending whose opening failed isn't drawn at all.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_ending_waits_for_the_opening_drawn_in_the_same_run() {
      let (base_url, requests) = fake_vertex(vec![image_reply(), image_reply()]);
      let (engine, dir) = engine(base_url);
      let (tx, rx) = std::sync::mpsc::channel();
      let ending = StillJob { index: 0, version: 2, image: "A face fills the frame".into(), guide: None, ending: true, opening: None, after: None, sing: false };
      let shots = vec![ending.clone(), StillJob { index: 0, version: 1, image: "A dock, far off".into(), guide: None, ending: false, opening: None, after: None, sing: false }, StillJob { index: 5, ..ending }];
      engine.start_song(21, SongRequest::Stills { dir: dir.join("p"), design: Design::default(), references: Vec::new(), aspect: Aspect::Wide, quality: Quality::Draft, better: false, shots }, tx, egui::Context::default());
      let updates = finished(&rx, 21).await;
      let stills: Vec<(usize, bool)> = updates.iter().filter_map(|u| if let SongUpdate::Still { index, ending, .. } = u { Some((*index, *ending)) } else { None }).collect();
      assert_eq!(stills, [(0, false), (0, true)], "{updates:?}");
      let requests = requests.lock().unwrap();
      assert_eq!(requests.len(), 2, "scene 6's ending has no opening to match, so it isn't drawn");
      let parts = serde_json::from_str::<Value>(&requests[1].1).unwrap()["contents"][0]["parts"].clone();
      assert!(parts[0]["inlineData"]["data"].is_string() && parts[1]["text"].as_str().unwrap().contains("This is the frame the shot ends on"), "the ending is drawn with the new opening beside it");
      let _ = std::fs::remove_dir_all(dir);
    }

    /// In a flowing sequence a scene opens on the ending frame of the one before, so each
    /// ending waits for the one it continues from and is drawn with it attached.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_flowing_scene_is_drawn_from_the_ending_before_it() {
      let (base_url, requests) = fake_vertex(vec![image_reply(), image_reply(), image_reply()]);
      let (engine, dir) = engine(base_url);
      let (tx, rx) = std::sync::mpsc::channel();
      let frame = |index, image: &str, ending, after| StillJob { index, version: if ending { 2 } else { 1 }, image: image.into(), guide: None, ending, opening: None, after, sing: false };
      // Scene 2 has no opening of its own: its ending follows scene 1's ending.
      let shots = vec![frame(1, "The quay, from the water", true, Some((0, true))), frame(0, "The deck at the rail", true, None), frame(0, "A dock, far off", false, None)];
      engine.start_song(22, SongRequest::Stills { dir: dir.join("p"), design: Design::default(), references: Vec::new(), aspect: Aspect::Wide, quality: Quality::Draft, better: false, shots }, tx, egui::Context::default());
      let updates = finished(&rx, 22).await;
      let stills: Vec<(usize, bool)> = updates.iter().filter_map(|u| if let SongUpdate::Still { index, ending, .. } = u { Some((*index, *ending)) } else { None }).collect();
      assert_eq!(stills, [(0, false), (0, true), (1, true)], "{updates:?}");
      let requests = requests.lock().unwrap();
      let text = |n: usize| serde_json::from_str::<Value>(&requests[n].1).unwrap()["contents"][0]["parts"].as_array().unwrap().iter().filter_map(|part| part["text"].as_str().map(str::to_owned)).collect::<String>();
      assert!(text(1).contains("The deck at the rail") && text(2).contains("The quay, from the water"));
      assert!(serde_json::from_str::<Value>(&requests[2].1).unwrap()["contents"][0]["parts"][0]["inlineData"]["data"].is_string(), "scene 1's ending is attached as scene 2's opening");
      let _ = std::fs::remove_dir_all(dir);
    }

    /// A draft still can be redrawn with the better model: Pro, still at the draft's size and name.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_draft_still_can_be_redrawn_with_the_better_model() {
      let (base_url, requests) = fake_vertex(vec![image_reply()]);
      let (engine, dir) = engine(base_url);
      let (tx, rx) = std::sync::mpsc::channel();
      let shots = vec![StillJob { index: 4, version: 2, image: "A wide shot".into(), guide: None, ending: false, opening: None, after: None, sing: false }];
      engine.start_song(12, SongRequest::Stills { dir: dir.join("p"), design: Design::default(), references: Vec::new(), aspect: Aspect::Wide, quality: Quality::Draft, better: true, shots }, tx, egui::Context::default());
      let updates = finished(&rx, 12).await;
      assert!(matches!(updates.first(), Some(SongUpdate::Still { index: 4, path, ending: false }) if path.ends_with("shots/shot_005_draft_v2.png")), "{updates:?}");
      let requests = requests.lock().unwrap();
      assert!(requests[0].0.contains("gemini-3-pro-image:generateContent"), "{}", requests[0].0);
      assert_eq!(serde_json::from_str::<Value>(&requests[0].1).unwrap()["generationConfig"]["imageConfig"]["imageSize"], "1K");
      let _ = std::fs::remove_dir_all(dir);
    }

    /// When Vertex is out of capacity for a picture and a Gemini API key is set, the picture is
    /// made through the Gemini API without waiting.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_picture_vertex_has_no_room_for_goes_to_the_gemini_api() {
      let refusal = format!("429:{}", json!({ "error": { "code": 429, "message": "Resource has been exhausted (e.g. check quota).", "status": "RESOURCE_EXHAUSTED" } }));
      let (vertex_url, vertex_requests) = fake_vertex(vec![refusal]);
      let (gemini_url, gemini_requests) = fake_vertex(vec![image_reply()]);
      let dir = std::env::temp_dir().join(format!("gesso-song-{}", uuid::Uuid::new_v4()));
      let config = LocalConfig { gcp_api_key: Some("test-key".into()), vertex_base_url: Some(vertex_url), gemini_api_key: Some("gm-key".into()), gemini_base_url: Some(gemini_url), bedrock_enabled: Some(false), ..Default::default() };
      let engine = Arc::new(LocalEngine::new(&dir, config, reqwest::Client::builder().no_proxy().build().unwrap()));
      let (tx, rx) = std::sync::mpsc::channel();
      let shots = vec![StillJob { index: 0, version: 1, image: "A wide shot".into(), guide: None, ending: false, opening: None, after: None, sing: false }];
      engine.start_song(11, SongRequest::Stills { dir: dir.join("p"), design: Design::default(), references: Vec::new(), aspect: Aspect::Wide, quality: Quality::Draft, better: false, shots }, tx, egui::Context::default());
      let updates = finished(&rx, 11).await;
      assert!(matches!(updates.first(), Some(SongUpdate::Still { index: 0, .. })), "{updates:?}");
      assert_eq!(vertex_requests.lock().unwrap().len(), 1);
      let gemini_requests = gemini_requests.lock().unwrap();
      assert!(gemini_requests[0].0.starts_with("POST /v1beta/models/gemini-3.1-flash-image:generateContent"), "{}", gemini_requests[0].0);
      assert!(gemini_requests[0].1.contains("A wide shot"));
      let _ = std::fs::remove_dir_all(dir);
    }

    /// Opening a video made before everything went to the library adds what is in its folder:
    /// every version of every picture, clip and track, but not the grid's small copies, the
    /// project file or anything already there.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_videos_older_files_are_added_to_the_library() {
      let (engine, dir) = engine("http://127.0.0.1:9/".to_owned());
      let project = dir.join("p");
      std::fs::create_dir_all(&project).unwrap();
      std::fs::write(project.join("project.json"), r#"{"title":"Context Maker"}"#).unwrap();
      let draft = save_picture(&project.join("shots"), "shot_012_draft_v3.png", &tiny_png()).unwrap();
      let final_still = save_picture(&project.join("shots"), "shot_012_final_v1.png", &tiny_png()).unwrap();
      let sheet = save(&project.join("references"), "look_01_v2.png", &tiny_png()).unwrap();
      let song = save(&project, "song_imported.mp3", b"mp3").unwrap();
      let known = BTreeSet::from([sheet.clone()]);
      let (tx, rx) = std::sync::mpsc::channel();
      engine.start_song(14, SongRequest::Catalogue { dir: project.clone(), known }, tx.clone(), egui::Context::default());
      let updates = finished(&rx, 14).await;
      let Some(SongUpdate::Catalogued(added)) = updates.iter().find(|u| matches!(u, SongUpdate::Catalogued(_))) else { panic!("{updates:?}") };
      assert_eq!(added, &[song.clone(), draft.clone(), final_still.clone()], "the sheet was there already; small copies and the project file aren't media");
      let titles: Vec<String> = engine.library().list("image", false, 0, 40).0.into_iter().filter_map(|file| file.maybe_title).collect();
      assert!(titles.contains(&"Context Maker \u{b7} scene 12 still, draft, v3".to_owned()) && titles.contains(&"Context Maker \u{b7} scene 12 still, final, v1".to_owned()), "{titles:?}");
      // A track the user brought in shows on the library page too (which leaves uploads out).
      let tracks = engine.library().list("audio", false, 0, 40).0;
      assert_eq!((tracks.len(), tracks[0].maybe_title.as_deref()), (1, Some("Context Maker \u{b7} song, imported")));

      // A second look finds nothing new.
      engine.start_song(15, SongRequest::Catalogue { dir: project, known: BTreeSet::from([sheet, song, draft, final_still]) }, tx, egui::Context::default());
      let again = finished(&rx, 15).await;
      assert!(again.iter().any(|u| matches!(u, SongUpdate::Catalogued(added) if added.is_empty())), "{again:?}");
      let _ = std::fs::remove_dir_all(dir);
    }

    /// Working on a section asks the writer once and returns its version of that section alone.
    #[tokio::test(flavor = "multi_thread")]
    async fn working_on_a_section_returns_the_writers_version_of_it() {
      let reply = json!({ "reply": "Lost the strobe.", "fields": { "name": "The Sump Vault", "description": "A flooded bunker under one lamp." } });
      let (base_url, requests) = fake_vertex(vec![text_reply(&reply.to_string(), 300, 60)]);
      let (engine, dir) = engine(base_url);
      let (tx, rx) = std::sync::mpsc::channel();
      let fields = vec![("name".to_owned(), "The Sump Vault".to_owned()), ("description".to_owned(), "A flooded bunker with a strobe.".to_owned())];
      let turns = vec![Turn { from_user: true, text: "Lose the strobe".into() }];
      engine.start_song(8, SongRequest::Workshop { context: "The film is \"Adrift\".".into(), section: "Venue".into(), fields, turns, writer: Writer::Gemini }, tx, egui::Context::default());
      let updates = finished(&rx, 8).await;
      let Some(SongUpdate::Workshopped(revision)) = updates.first() else { panic!("{updates:?}") };
      assert_eq!((revision.reply.as_str(), revision.fields[1].1.as_str()), ("Lost the strobe.", "A flooded bunker under one lamp."));
      let requests = requests.lock().unwrap();
      assert_eq!(requests.len(), 1);
      let body: Value = serde_json::from_str(&requests[0].1).unwrap();
      assert!(body["systemInstruction"]["parts"][0]["text"].as_str().unwrap().starts_with("You are a script editor"));
      assert!(body["contents"][0]["parts"][0]["text"].as_str().unwrap().contains("The director now says:\nLose the strobe"));
      let _ = std::fs::remove_dir_all(dir);
    }

    /// With a Gemini API key, the song goes to the Gemini API (Lyria 3.5 first), not Vertex.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_gemini_api_key_takes_the_song_off_vertex() {
      let song = json!({ "status": "completed", "steps": [{ "type": "model_output", "content": [{ "type": "text", "text": "[Verse]" }, { "type": "audio", "mime_type": "audio/mpeg", "data": "AQID" }] }] });
      let (gemini_url, requests) = fake_vertex(vec![song.to_string()]);
      let dir = std::env::temp_dir().join(format!("gesso-song-{}", uuid::Uuid::new_v4()));
      let config = LocalConfig { gcp_project: Some("p".into()), gemini_api_key: Some("gm-key".into()), gemini_base_url: Some(gemini_url), vertex_base_url: Some("http://127.0.0.1:9/".into()), bedrock_enabled: Some(false), ..Default::default() };
      let engine = Arc::new(LocalEngine::new(&dir, config, reqwest::Client::builder().no_proxy().build().unwrap()));
      let (tx, rx) = std::sync::mpsc::channel();
      let draft = Draft { title: "Night Bus".into(), style: "slow, 90 BPM".into(), lyrics: "[Verse]\nline one".into() };
      engine.start_song_for(4, Some("song_a".into()), SongRequest::Compose { dir: dir.join("p"), draft, length: SongLength::TwoMinutes, instrumental: false, version: 1 }, tx, egui::Context::default());
      let updates = finished(&rx, 4).await;
      assert!(matches!(updates.last(), Some(SongUpdate::Done(Ok(())))), "{updates:?}");
      assert!(updates.iter().any(|u| matches!(u, SongUpdate::Track { model, path, .. } if model == "lyria-3.5" && std::fs::read(path).unwrap() == [1, 2, 3])), "{updates:?}");
      // The song is counted towards the video it was made for.
      assert_eq!(engine.spend_by_project().get("song_a"), Some(&vec![("Song".to_owned(), 0.08)]));
      // It came through the key, so it is the key's balance that paid, not the Cloud project.
      let all_time = engine.spend.summary().all_time;
      assert_eq!((all_time.gemini_api, all_time.vertex), (0.08, 0.0));
      let requests = requests.lock().unwrap();
      assert_eq!(requests[0].0, "POST /v1beta/interactions HTTP/1.1");
      let body: Value = serde_json::from_str(&requests[0].1).unwrap();
      assert_eq!((body["model"].clone(), body["store"].clone(), body.get("background")), (json!("lyria-3.5"), json!(false), None));
      let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stopped_run_makes_nothing_more() {
      let (base_url, requests) = fake_vertex(Vec::new());
      let (engine, dir) = engine(base_url);
      let run = SongRun { engine: &engine, run_id: 9, tx: &std::sync::mpsc::channel().0, ctx: &egui::Context::default(), cancel: Arc::new(AtomicBool::new(true)), batch: "b".into(), pace: Pace::new(Duration::ZERO), project: None, part: COST_PARTS[0] };
      let shots = vec![StillJob { index: 0, version: 1, image: "x".into(), guide: None, ending: false, opening: None, after: None, sing: false }];
      run.stage(SongRequest::Stills { dir: dir.join("p"), design: Design::default(), references: Vec::new(), aspect: Aspect::Wide, quality: Quality::Draft, better: false, shots }).await.unwrap();
      assert!(requests.lock().unwrap().is_empty());
      let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_stage_reports_why() {
      let (engine, dir) = engine("http://127.0.0.1:9/".to_owned());
      let (tx, rx) = std::sync::mpsc::channel();
      engine.start_song(2, SongRequest::Import { dir: dir.join("p"), file: dir.join("missing.mp3") }, tx, egui::Context::default());
      let updates = finished(&rx, 2).await;
      assert!(matches!(updates.last(), Some(SongUpdate::Done(Err(message))) if message.contains("missing.mp3")));
      let _ = std::fs::remove_dir_all(dir);
    }
  }

  mod prompts {
    use super::*;

    #[test]
    fn the_music_prompt_leads_with_the_direction() {
      let draft = Draft { title: "T".into(), style: "Fast breakbeat, 140 BPM".into(), lyrics: "[Verse]\nla".into() };
      let full = music_prompt(&draft, SongLength::TwoMinutes, false, Wording::Sorted);
      assert!(full.starts_with("Musical style, to follow closely for the genre, tempo, instruments, vocal and production, whatever the lyrics suggest:\nFast breakbeat, 140 BPM\n\nA complete song about two minutes long"), "{full}");
      assert!(music_prompt(&draft, SongLength::TwoMinutes, false, Wording::AsTyped).starts_with("Fast breakbeat, 140 BPM\nA complete song about two minutes long"));
      assert!(music_prompt(&Draft { style: " ".into(), ..draft.clone() }, SongLength::TwoMinutes, false, Wording::Sorted).starts_with("A complete song"));
      assert!(full.ends_with("[Verse]\nla"));
      // Directions written among the lyrics go to the band, not the singer.
      let noted = Draft { lyrics: "[Intro: guitar loop, drums kick in]\nVerse 1\nla\nOutro [repeat to fade]".into(), ..draft.clone() };
      let sent = music_prompt(&noted, SongLength::Minute, false, Wording::Sorted);
      assert!(sent.contains("Arrangement notes, for the instruments and production only, never sung or spoken: Intro: guitar loop, drums kick in; Outro: repeat to fade."), "{sent}");
      assert!(sent.ends_with("never sing, speak or read it out.\n\n[Intro]\n\n[Verse 1]\nla\n\n[Outro]"), "{sent}");
      assert!(!full.contains("Arrangement notes"));
      // After a refusal: the same lyrics without the notes, then exactly as typed.
      let plain = music_prompt(&noted, SongLength::Minute, false, Wording::WithoutNotes);
      assert!(!plain.contains("Arrangement notes") && !plain.contains("guitar loop") && plain.ends_with("[Outro]"), "{plain}");
      assert!(music_prompt(&noted, SongLength::Minute, false, Wording::AsTyped).ends_with("following their section tags:\n\n[Intro: guitar loop, drums kick in]\nVerse 1\nla\nOutro [repeat to fade]"));
      assert!(is_blocked(&anyhow::anyhow!("Vertex AI error 400 Bad Request: Input blocked: The prompt could not be submitted. The prompt contains sensitive words")) && !is_blocked(&anyhow::anyhow!("429 Too Many Requests")));
      assert!(music_prompt(&draft, SongLength::Minute, false, Wording::Sorted).contains("about one minute long") && music_prompt(&draft, SongLength::ThreeMinutes, false, Wording::Sorted).contains("about three minutes long"));
      let instrumental = music_prompt(&draft, SongLength::Clip, true, Wording::Sorted);
      assert!(instrumental.contains("A 30-second piece.") && instrumental.ends_with("Instrumental only, no vocals."));
      assert!(!instrumental.contains("la"));
    }

    #[test]
    fn stills_say_what_the_attached_pictures_are() {
      let design = Design { look: "grainy black and white".into(), character: "a dancer in a lanyard".into(), venue: String::new() };
      let sheet_only = still_prompt(&design, "A tunnel", &[ReferenceKind::Look], false, false, false);
      assert!(sheet_only.contains("The attached picture is the performer's reference sheet.") && sheet_only.contains("they are the person on the reference sheet"));
      assert!(!sheet_only.contains("Keep the location"));
      let all = still_prompt(&design, "A tunnel", &[ReferenceKind::Look, ReferenceKind::Location, ReferenceKind::Prop], false, false, false);
      assert!(all.contains("The attached pictures, in order: 1, the performer's reference sheet; 2, the location; 3, a prop.") && all.contains("Keep the location and the props as their pictures show them."));
      let alone = still_prompt(&design, "A tunnel", &[], false, false, false);
      assert_eq!(alone, "A single frame from a music video. A tunnel. The performer: a dancer in a lanyard. The video's look, which this frame must match: grainy black and white. Strictly black and white: no colour anywhere in the frame. No text, captions, subtitles, borders or logos.");
      assert_eq!(clip_prompt("grainy", "She runs", false, false), "She runs. Keep the look of the opening frame: grainy. No text or captions appear.");
      // A sung scene's frames and clip keep the mouth clear: no microphone or anything in front of it.
      let sung = still_prompt(&design, "A tunnel", &[], false, false, true);
      assert!(sung.contains("No microphone anywhere near the mouth") && sung.contains("nothing at all in front of or touching the lower face"), "{sung}");
      assert!(!still_prompt(&design, "A tunnel", &[], false, false, false).contains("microphone"));
      assert!(clip_prompt("grainy", "She turns", true, false).contains("no microphone (the performer holds none"));
    }

    #[test]
    fn a_black_and_white_look_is_said_outright_and_enforced() {
      let design = Design { look: "Raw 16mm Black-and-White film".into(), character: "a dancer".into(), venue: String::new() };
      assert!(still_prompt(&design, "A tunnel", &[], false, false, false).contains("film. Strictly black and white: no colour anywhere in the frame. No text"));
      assert!(clip_prompt(&design.look, "She runs", false, false).contains("Strictly black and white"));
      assert!(reference_prompt(ReferenceKind::Prop, "Lanyard", "A badge", "monochrome, grainy").contains("Strictly black and white"));
      assert!(!is_monochrome("neon colour, wet streets"));
      let mut red = Vec::new();
      image::RgbImage::from_pixel(4, 4, image::Rgb([220, 20, 20])).write_to(&mut std::io::Cursor::new(&mut red), image::ImageFormat::Png).unwrap();
      let kept = in_keeping(GeneratedFile { bytes: red.clone(), extension: "png", usage: None, via_key: false }, "neon colour");
      assert_eq!(kept.bytes, red, "a colour look is left alone");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_colour_picture_for_a_black_and_white_film_loses_its_colour() {
      let mut red = Vec::new();
      image::RgbImage::from_pixel(4, 4, image::Rgb([220, 20, 20])).write_to(&mut std::io::Cursor::new(&mut red), image::ImageFormat::Jpeg).unwrap();
      let grey = in_keeping(GeneratedFile { bytes: red, extension: "jpg", usage: None, via_key: false }, "grainy black and white");
      assert_eq!(grey.extension, "png");
      let pixel = image::load_from_memory(&grey.bytes).unwrap().to_rgb8().get_pixel(1, 1).0;
      assert!(pixel[0] == pixel[1] && pixel[1] == pixel[2], "{pixel:?}");
    }

    #[test]
    fn the_close_up_is_named_and_the_face_is_taken_from_it() {
      let design = Design { look: "grainy".into(), character: "a dancer".into(), venue: String::new() };
      let both = still_prompt(&design, "A tunnel", &[ReferenceKind::Look, ReferenceKind::Face, ReferenceKind::Location], false, false, false);
      assert!(both.contains("in order: 1, the performer's reference sheet; 2, the performer's face in close-up; 3, the location."), "{both}");
      assert!(both.contains("Take the face from the close-up, which shows it in full detail"));
      let face_only = still_prompt(&design, "A tunnel", &[ReferenceKind::Face], false, false, false);
      assert!(face_only.contains("they are the person in the close-up") && !face_only.contains("Keep the location"), "{face_only}");
      assert!(face_from_sheet_prompt("grainy black and white").contains("this exact person") && face_from_sheet_prompt("grainy black and white").contains("Strictly black and white"));
    }

    #[test]
    fn library_titles_say_what_each_file_is() {
      let named = |stem: &str| describe_asset(stem);
      assert_eq!(named("shot_012_final_v3"), "scene 12 still, final, v3");
      assert_eq!(named("clip_007_draft_synced_v2"), "scene 7 clip, draft, lip-synced, v2");
      assert_eq!(named("shot_003_v1"), "scene 3 still, v1", "a still from before there were two passes");
      assert_eq!((named("look_01_v2"), named("face_02_v1"), named("prop_04_v1")), ("performer, v2".to_owned(), "face, v1".to_owned(), "prop, v1".to_owned()));
      assert_eq!((named("song_v483920"), named("vocals"), named("vocals_imported"), named("cut_v4")), ("song, v483920".to_owned(), "vocal stem".to_owned(), "vocal stem, imported".to_owned(), "the film, v4".to_owned()));
    }

    #[test]
    fn a_shot_with_an_ending_frame_is_told_to_land_on_it() {
      let ends = clip_prompt("grainy", "The camera pushes in", false, true);
      assert!(ends.contains("ends exactly on the last frame"), "{ends}");
      assert!(!clip_prompt("grainy", "The camera pushes in", false, false).contains("last frame"));
      let design = Design { look: "grainy".into(), character: "a dancer".into(), venue: String::new() };
      // In the final pass the draft ending goes first, then the opening frame, then the references.
      let final_ending = still_prompt(&design, "A face", &[ReferenceKind::Look, ReferenceKind::Face], true, true, false);
      assert!(final_ending.contains("in order: 1, the approved draft of this exact frame; 2, the frame this same shot opens on; 3, the performer's reference sheet; 4, the performer's face in close-up."), "{final_ending}");
      assert!(!still_prompt(&design, "A face", &[ReferenceKind::Look], false, false, false).contains("opens on"));
    }

    /// With the sheet missing, the location's picture must not be taken for the performer.
    #[test]
    fn a_missing_sheet_isnt_mistaken_for_the_location() {
      let design = Design { look: "grainy".into(), character: "a dancer".into(), venue: String::new() };
      let prompt = still_prompt(&design, "A tunnel", &[ReferenceKind::Location], false, false, false);
      assert!(prompt.contains("The attached picture is the location.") && prompt.contains("The performer: a dancer."));
      assert!(!prompt.contains("reference sheet"));
    }

    #[test]
    fn the_final_pass_recreates_the_approved_frame() {
      let design = Design { look: "grainy".into(), character: "a dancer".into(), venue: String::new() };
      let guided = still_prompt(&design, "A tunnel", &[ReferenceKind::Look, ReferenceKind::Location], true, false, false);
      assert!(guided.contains("in order: 1, the approved draft of this exact frame; 2, the performer's reference sheet; 3, the location."));
      assert!(guided.contains("Recreate the approved draft faithfully"));
      assert!(guided.contains("whatever this scene's wording or the draft suggests"), "the sheet outranks a draft that drifted");
      assert!(!still_prompt(&design, "A tunnel", &[ReferenceKind::Look, ReferenceKind::Location], false, false, false).contains("approved draft"));
    }

    #[test]
    fn sung_scenes_are_filmed_ready_for_lip_sync() {
      let sung = clip_prompt("grainy", "She leans in", true, false);
      assert!(sung.starts_with("She leans in. The performer sings to the camera throughout, mouth moving and clearly visible"));
      assert!(!clip_prompt("grainy", "She leans in", false, false).contains("sings"));
    }

    #[test]
    fn props_are_shot_alone_on_white() {
      let prompt = reference_prompt(ReferenceKind::Prop, "Lanyard", "A cracked badge", "grainy");
      assert!(prompt.contains("a single prop") && prompt.contains("The prop, Lanyard: A cracked badge.") && prompt.ends_with("grainy. No text, captions or logos."));
    }
  }

  mod pricing {
    use super::*;

    #[test]
    fn stages_are_priced_before_they_run() {
      let (engine, dir) = engine("http://127.0.0.1:9/".to_owned());
      assert_eq!(engine.estimate_song(&SongEstimate::Song { clip: false }), 0.08);
      assert_eq!(engine.estimate_song(&SongEstimate::Song { clip: true }), 0.04);
      // 1,120 image tokens a still at $60 a million.
      assert!((engine.estimate_song(&SongEstimate::Stills { count: 10, pro: false, large: false }) - 10.0 * 1120.0 * 60.0 / 1e6).abs() < 1e-9);
      let final_stills = engine.estimate_song(&SongEstimate::Stills { count: 10, pro: true, large: true });
      // Nano Banana Pro at $120 a million image tokens: twice the draft's price.
      assert!((final_stills - 10.0 * 1120.0 * 120.0 / 1e6).abs() < 1e-9, "{final_stills}");
      // A minute of lip sync at $3, plus separating a three-minute track.
      let sync = engine.estimate_song(&SongEstimate::LipSync { seconds: 60.0, pro: false, separate_seconds: 180.0 });
      assert!((sync - (3.0 + 180.0 * 0.0007)).abs() < 1e-9);
      assert!((engine.estimate_song(&SongEstimate::LipSync { seconds: 60.0, pro: true, separate_seconds: 0.0 }) - 5.0).abs() < 1e-9);
      let clips = |tier, resolution: &str| engine.estimate_song(&SongEstimate::Clips { seconds: 100, tier, resolution: resolution.to_owned() });
      assert!((clips(VeoTier::Fast, "720p") - 8.0).abs() < 1e-9);
      assert!((clips(VeoTier::Lite, "720p") - 3.0).abs() < 1e-9);
      assert!((clips(VeoTier::Standard, "1080p") - 20.0).abs() < 1e-9);
      let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn quota_errors_are_retried_and_others_arent() {
      let pace = Pace::new(Duration::ZERO);
      let (calls, waits) = (Mutex::new(0), Mutex::new(0));
      let result = with_quota_retries(&pace, || true, |_| *waits.lock().unwrap() += 1, || async {
        let mut n = calls.lock().unwrap();
        *n += 1;
        if *n < 3 {
          Err(anyhow::anyhow!("Vertex AI error 429 Too Many Requests: quota exceeded"))
        } else {
          Ok(*n)
        }
      })
      .await;
      assert_eq!((result.unwrap(), *waits.lock().unwrap()), (3, 2), "the page is told about each wait");
      let calls = Mutex::new(0);
      let result: anyhow::Result<()> = with_quota_retries(&pace, || true, |_| {}, || async {
        *calls.lock().unwrap() += 1;
        Err(anyhow::anyhow!("Vertex AI error 400 Bad Request: bad prompt"))
      })
      .await;
      assert!(result.is_err());
      assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn a_busy_service_is_waited_for_like_a_full_quota() {
      assert!(is_quota(&anyhow::anyhow!("Veo failed: The service is currently experiencing high load and cannot process your request. Please try again later. Operation ID: 14a0a7ca.")));
      assert!(is_quota(&anyhow::anyhow!("Vertex AI error 503 Service Unavailable: backend")));
      assert!(is_quota(&anyhow::anyhow!("inner: RESOURCE_EXHAUSTED").context("outer")));
      assert!(!is_quota(&anyhow::anyhow!("Veo failed: the prompt was blocked by safety filters")));
    }

    #[tokio::test]
    async fn a_full_quota_gives_up_in_the_end_and_says_what_to_do() {
      let pace = Pace::new(Duration::ZERO);
      let calls = Mutex::new(0);
      let result: anyhow::Result<()> = with_quota_retries(&pace, || true, |_| {}, || async {
        *calls.lock().unwrap() += 1;
        Err(anyhow::anyhow!("Vertex AI error 429 Too Many Requests: Resource has been exhausted (e.g. check quota)."))
      })
      .await;
      let message = format!("{:#}", result.unwrap_err());
      assert_eq!(*calls.lock().unwrap(), QUOTA_RETRIES + 1);
      assert!(message.contains("only the ones still missing") && message.contains("error 429"), "{message}");
    }

    #[tokio::test]
    async fn a_refusal_holds_every_call_and_spaces_them_out_and_stop_ends_the_wait() {
      let pace = Pace::new(Duration::from_secs(20));
      pace.slow_down(Duration::from_millis(60)).await;
      let began = tokio::time::Instant::now();
      assert!(pace.turn(&|| true).await);
      assert!(began.elapsed() >= Duration::from_millis(55), "the held call waited");
      assert_eq!(pace.state.lock().await.gap, QUOTA_FIRST_GAP);
      pace.slow_down(Duration::from_secs(600)).await;
      assert_eq!(pace.state.lock().await.gap, QUOTA_FIRST_GAP * 2);
      let began = tokio::time::Instant::now();
      let stopped: anyhow::Result<()> = with_quota_retries(&pace, || false, |_| {}, || async { Ok(()) }).await;
      assert!(stopped.unwrap_err().to_string().contains("Stopped") && began.elapsed() < Duration::from_secs(5));
    }
  }

  fn engine(base_url: String) -> (Arc<LocalEngine>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("gesso-song-{}", uuid::Uuid::new_v4()));
    let config = LocalConfig { gcp_api_key: Some("test-key".into()), vertex_base_url: Some(base_url), bedrock_enabled: Some(false), ..Default::default() };
    (Arc::new(LocalEngine::new(&dir, config, reqwest::Client::builder().no_proxy().build().unwrap())), dir)
  }

  /// Every update of run `run_id`, up to and including `Done`.
  async fn finished(rx: &Receiver<Event>, run_id: u64) -> Vec<SongUpdate> {
    let mut updates = Vec::new();
    for _ in 0..400 {
      for event in rx.try_iter() {
        if let Event::Song { run_id: id, update } = event {
          assert_eq!(id, run_id);
          updates.push(update);
        }
      }
      if matches!(updates.last(), Some(SongUpdate::Done(_))) {
        return updates;
      }
      tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("run {run_id} never finished: {updates:?}");
  }

  fn slot(index: usize, energy: Energy) -> Slot {
    Slot { index, start: index as f64 * 2.0, end: index as f64 * 2.0 + 2.0, section: "Verse".to_owned(), energy, lyric: String::new() }
  }

  fn text_reply(text: &str, prompt_tokens: u64, output_tokens: u64) -> String {
    json!({ "candidates": [{ "finishReason": "STOP", "content": { "parts": [{ "text": text }] } }], "usageMetadata": { "promptTokenCount": prompt_tokens, "candidatesTokenCount": output_tokens } }).to_string()
  }

  fn image_reply() -> String {
    json!({ "candidates": [{ "finishReason": "STOP", "content": { "parts": [{ "inlineData": { "mimeType": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(tiny_png()) } }] } }] }).to_string()
  }

  fn tiny_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 80, 40, 255])).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    bytes
  }

  /// A one-thread HTTP server that answers with `replies` in order (one that starts `429:` as a
  /// quota refusal), recording each request line and body.
  fn fake_vertex(replies: Vec<String>) -> (String, Arc<Mutex<Vec<(String, String)>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    std::thread::spawn(move || {
      for (stream, reply) in listener.incoming().zip(replies) {
        let mut stream = stream.unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let mut length = 0;
        loop {
          let mut line = String::new();
          reader.read_line(&mut line).unwrap();
          if line == "\r\n" {
            break;
          }
          if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap();
          }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        record.lock().unwrap().push((request_line.trim().to_owned(), String::from_utf8(body).unwrap()));
        // A reply that starts "429:" is sent as a quota refusal.
        let (status, reply) = reply.strip_prefix("429:").map_or(("200 OK", reply.as_str()), |body| ("429 Too Many Requests", body));
        write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).unwrap();
      }
    });
    (url, seen)
  }
}
