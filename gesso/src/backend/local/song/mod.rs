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

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::stream;
use log::{info, warn};

use super::bedrock::DEFAULT_CLAUDE_MODEL;
use super::library::{LocalLibrary, MediaClass, NewFile, Origin};
use super::spend::Account;
use super::tools::VeoTier;
use super::vertex::{GeminiImageRequest, SONG_CLIP_MODEL, SONG_MODELS, TEXT_MODELS, TextRequest, VeoRequest};
use super::{LocalEngine, RefImage};
use crate::backend::Event;
use beats::BeatGrid;
use cut::{Aspect, CutShot, Grade};
use plan::{Section, Slot};
use project::ReferenceKind;
use writer::{Brief, Design, Draft, PlannedShot, Read, Style, TimedLine};

/// Stills made at once (each is one Gemini call).
const STILLS_AT_ONCE: usize = 4;
/// Clips made at once (Veo's per-minute quota is small on new projects).
const CLIPS_AT_ONCE: usize = 3;
/// Waits before retrying a call the provider turned away for quota: 20 s, 40 s, 80 s.
const QUOTA_RETRIES: u32 = 3;
const QUOTA_WAIT: Duration = Duration::from_secs(20);
/// Stills are storyboard frames and Veo's starting images: 1K is plenty.
const STILL_SIZE: &str = "1K";
/// A still is drawn from at most this many reference images (the performer first).
const REFERENCES_PER_STILL: usize = 5;
/// The performer's sheet shows them three times side by side; props are shot square.
const LOOK_RATIO: &str = "16:9";
const PROP_RATIO: &str = "1:1";
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
}

/// A still to make.
#[derive(Clone, Debug, PartialEq)]
pub struct StillJob {
  pub index: usize,
  pub version: u32,
  pub image: String,
}

/// A clip to make from a still.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipJob {
  pub index: usize,
  pub version: u32,
  pub still: PathBuf,
  pub motion: String,
  /// The slot's length; the clip is made at the shortest length Veo offers that covers it.
  pub seconds: f64,
}

/// One stage's run. `dir` is the project's folder.
#[derive(Clone, Debug, PartialEq)]
pub enum SongRequest {
  /// A song from an idea, or a rewrite of `current` following `notes`.
  Write {
    idea: String,
    notes: String,
    current: Option<Draft>,
    writer: Writer,
  },
  /// The song itself, from Lyria. `clip` makes a 30-second clip instead of a full song.
  Compose {
    dir: PathBuf,
    draft: Draft,
    clip: bool,
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
  /// A picture and a movement for every slot.
  Plan {
    brief: Brief,
    style: Style,
    slots: Vec<Slot>,
    writer: Writer,
  },
  /// Each scene's opening frame, drawn from `references` (the performer's sheet first).
  Stills {
    dir: PathBuf,
    design: Design,
    references: Vec<PathBuf>,
    aspect: Aspect,
    pro: bool,
    shots: Vec<StillJob>,
  },
  Clips {
    dir: PathBuf,
    look: String,
    aspect: Aspect,
    tier: VeoTier,
    resolution: String,
    shots: Vec<ClipJob>,
  },
  Cut {
    dir: PathBuf,
    audio: PathBuf,
    aspect: Aspect,
    resolution: String,
    grade: Grade,
    shots: Vec<CutShot>,
    version: u32,
  },
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
  Reference {
    index: usize,
    path: PathBuf,
  },
  ReferenceFailed {
    index: usize,
    message: String,
  },
  Planned(Vec<PlannedShot>),
  Still {
    index: usize,
    path: PathBuf,
  },
  Clip {
    index: usize,
    path: PathBuf,
  },
  ShotFailed {
    index: usize,
    message: String,
  },
  Cut(PathBuf),
  /// A progress line for the page.
  Note(String),
  Done(Result<(), String>),
}

/// Something the page prices before running it.
#[derive(Clone, Debug, PartialEq)]
pub enum SongEstimate {
  Song {
    clip: bool,
  },
  /// Reference images or stills: `count` pictures.
  Stills {
    count: usize,
    pro: bool,
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
  pub fn start_song(self: &Arc<Self>, run_id: u64, request: SongRequest, tx: Sender<Event>, ctx: egui::Context) {
    let cancel = Arc::new(AtomicBool::new(false));
    if let Ok(mut runs) = self.song_runs.lock() {
      runs.insert(run_id, cancel.clone());
    }
    let engine = self.clone();
    tokio::spawn(async move {
      let run = SongRun { engine: &engine, run_id, tx: &tx, ctx: &ctx, cancel, batch: LocalLibrary::new_batch_token() };
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
      SongEstimate::Song { clip } => self.prices.lyria_song(if *clip { SONG_CLIP_MODEL } else { SONG_MODELS[0] }),
      SongEstimate::Stills { count, pro } => *count as f64 * self.prices.gemini_call(still_model(*pro), None, Some(STILL_SIZE)).unwrap_or(0.0),
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
}

impl SongRun<'_> {
  async fn stage(&self, request: SongRequest) -> anyhow::Result<()> {
    match request {
      SongRequest::Write { idea, notes, current, writer } => {
        let reply = self.write(writer, writer::SONGWRITER, &writer::song_prompt(&idea, &notes, current.as_ref())).await?;
        self.send(SongUpdate::Written(writer::parse_draft(&reply)?));
      },
      SongRequest::Compose { dir, draft, clip, instrumental, version } => self.compose(&dir, &draft, clip, instrumental, version).await?,
      SongRequest::Import { dir, file } => {
        let extension = file.extension().and_then(|e| e.to_str()).unwrap_or("mp3").to_ascii_lowercase();
        let path = dir.join(format!("song_imported.{extension}"));
        std::fs::create_dir_all(&dir)?;
        std::fs::copy(&file, &path).map_err(|e| anyhow::anyhow!("Couldn't copy {}: {e}", file.display()))?;
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
      SongRequest::Plan { brief, style, slots, writer } => {
        let reply = self.write(writer, writer::DIRECTOR, &writer::scenes_prompt(&brief, &style, &slots)).await?;
        let written = writer::parse_scenes(&reply);
        let missing = slots.iter().filter(|slot| !written.iter().any(|scene| scene.index == slot.index)).count();
        if missing > 0 {
          self.send(SongUpdate::Note(format!("The writer left out {missing} of {} scenes; they have stand-in descriptions you can edit.", slots.len())));
        }
        let scenes = slots.iter().map(|slot| written.iter().find(|scene| scene.index == slot.index).cloned().unwrap_or_else(|| writer::fallback_scene(slot))).collect();
        self.send(SongUpdate::Planned(scenes));
      },
      SongRequest::Stills { dir, design, references, aspect, pro, shots } => {
        let references: Vec<RefImage> = references.iter().take(REFERENCES_PER_STILL).map(|path| self.engine.load_path(path)).collect::<anyhow::Result<_>>()?;
        let (dir, design, references) = (&dir.join("shots"), &design, &references);
        let jobs = shots.into_iter().map(|job| async move {
          if self.stopped() {
            return;
          }
          let index = job.index;
          match self.still(dir, design, references, aspect, pro, job).await {
            Ok(path) => self.send(SongUpdate::Still { index, path }),
            Err(err) => self.send(SongUpdate::ShotFailed { index, message: format!("{err:#}") }),
          }
        });
        stream::iter(jobs).buffer_unordered(STILLS_AT_ONCE).collect::<Vec<()>>().await;
      },
      SongRequest::Clips { dir, look, aspect, tier, resolution, shots } => {
        let (dir, look, resolution) = (&dir.join("clips"), look.as_str(), resolution.as_str());
        let jobs = shots.into_iter().map(|job| async move {
          if self.stopped() {
            return;
          }
          let index = job.index;
          match self.clip(dir, look, aspect, tier, resolution, job).await {
            Ok(path) => self.send(SongUpdate::Clip { index, path }),
            Err(err) => self.send(SongUpdate::ShotFailed { index, message: format!("{err:#}") }),
          }
        });
        stream::iter(jobs).buffer_unordered(CLIPS_AT_ONCE).collect::<Vec<()>>().await;
      },
      SongRequest::Cut { dir, audio, aspect, resolution, grade, shots, version } => {
        std::fs::create_dir_all(&dir)?;
        let out = dir.join(format!("cut_v{version}.mp4"));
        let target = out.clone();
        tokio::task::spawn_blocking(move || cut::assemble(&shots, &audio, aspect.size(&resolution), grade, &target)).await??;
        let bytes = std::fs::read(&out)?;
        self.add_to_library(&bytes, "mp4", MediaClass::Video);
        self.send(SongUpdate::Cut(out));
      },
    }
    Ok(())
  }

  async fn compose(&self, dir: &Path, draft: &Draft, clip: bool, instrumental: bool, version: u32) -> anyhow::Result<()> {
    let configured = self.engine.config.song_music_model.as_deref();
    let models: Vec<&str> = match (clip, configured) {
      (true, _) => vec![SONG_CLIP_MODEL],
      (false, Some(model)) => vec![model],
      (false, None) => SONG_MODELS.to_vec(),
    };
    let song = self.engine.vertex.generate_song(&models, &music_prompt(draft, clip, instrumental)).await?;
    info!("Song made by {}", song.model);
    self.charge(Account::Vertex, self.engine.prices.lyria_song(&song.model), &song.model);
    let path = save(dir, &format!("song_v{version}.{}", song.file.extension), &song.file.bytes)?;
    self.add_to_library(&song.file.bytes, song.file.extension, MediaClass::Audio);
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
    let prompt = reference_prompt(job.kind, &job.name, &job.description, look);
    let ratio = match job.kind {
      ReferenceKind::Look => LOOK_RATIO,
      ReferenceKind::Location => aspect.ratio(),
      ReferenceKind::Prop => PROP_RATIO,
    };
    let file = with_quota_retries(QUOTA_WAIT, || self.engine.vertex.generate_image(GeminiImageRequest { model, prompt: &prompt, images: &[], aspect_ratio: Some(ratio), image_size: Some(STILL_SIZE) })).await?;
    self.charge(Account::Vertex, self.engine.prices.gemini_call(model, file.usage, Some(STILL_SIZE)).unwrap_or(0.0), model);
    let path = save_picture(dir, &format!("{}_{:02}_v{}.{}", job.kind.label(), job.index + 1, job.version, file.extension), &file.bytes)?;
    self.add_to_library(&file.bytes, file.extension, MediaClass::Image);
    Ok(path)
  }

  async fn still(&self, dir: &Path, design: &Design, references: &[RefImage], aspect: Aspect, pro: bool, job: StillJob) -> anyhow::Result<PathBuf> {
    let model = still_model(pro);
    let prompt = still_prompt(design, &job.image, references.len());
    let file = with_quota_retries(QUOTA_WAIT, || self.engine.vertex.generate_image(GeminiImageRequest { model, prompt: &prompt, images: references, aspect_ratio: Some(aspect.ratio()), image_size: Some(STILL_SIZE) })).await?;
    self.charge(Account::Vertex, self.engine.prices.gemini_call(model, file.usage, Some(STILL_SIZE)).unwrap_or(0.0), model);
    save_picture(dir, &format!("shot_{:03}_v{}.{}", job.index + 1, job.version, file.extension), &file.bytes)
  }

  async fn clip(&self, dir: &Path, look: &str, aspect: Aspect, tier: VeoTier, resolution: &str, job: ClipJob) -> anyhow::Result<PathBuf> {
    let still = self.engine.load_path(&job.still)?;
    let seconds = if resolution == "1080p" { HD_CLIP_SECONDS } else { plan::clip_seconds(job.seconds) };
    let prompt = clip_prompt(look, &job.motion);
    let files = with_quota_retries(QUOTA_WAIT, || self.engine.vertex.generate_video(VeoRequest { model: tier.model(), prompt: &prompt, start_frame: Some(&still), end_frame: None, aspect_ratio: Some(aspect.ratio()), resolution: Some(resolution), duration_seconds: Some(seconds), sample_count: 1, generate_audio: Some(false), extend: None })).await?;
    let rate = self.engine.prices.veo_per_second(tier.model(), false, resolution).unwrap_or(0.0);
    self.charge(Account::Vertex, f64::from(seconds) * rate, tier.model());
    let file = files.into_iter().next().ok_or_else(|| anyhow::anyhow!("Veo returned no video"))?;
    save(dir, &format!("clip_{:03}_v{}.mp4", job.index + 1, job.version), &file.bytes)
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

  fn add_to_library(&self, bytes: &[u8], extension: &str, class: MediaClass) {
    let added = tokio::task::block_in_place(|| self.engine.library.add(NewFile { bytes, extension, class, origin: Origin::Generated, maybe_batch_token: Some(&self.batch), maybe_prompt_token: None, maybe_model_id: Some(MODEL_ID) }));
    if let Err(err) = added {
      warn!("Couldn't add the file to the library: {err:#}");
    }
  }

  fn charge(&self, account: Account, usd: f64, item: &str) {
    self.engine.charge(self.tx, vec![(account, usd, item.to_owned())], &self.batch);
  }

  fn stopped(&self) -> bool {
    self.cancel.load(Ordering::Relaxed)
  }

  fn send(&self, update: SongUpdate) {
    let _ = self.tx.send(Event::Song { run_id: self.run_id, update });
    self.ctx.request_repaint();
  }
}

/// What Lyria is given: the direction first, then the lyrics under their own heading.
fn music_prompt(draft: &Draft, clip: bool, instrumental: bool) -> String {
  let length = if clip { "A 30-second piece." } else { "A complete song, two to three minutes long, with an intro and an ending." };
  let mut prompt = format!("{}\n{length}", draft.style.trim());
  if instrumental || draft.lyrics.trim().is_empty() {
    prompt.push_str("\nInstrumental only, no vocals.");
  } else {
    prompt.push_str(&format!("\n\nSing these lyrics as written, following their section tags:\n\n{}", draft.lyrics.trim()));
  }
  prompt
}

fn reference_prompt(kind: ReferenceKind, name: &str, description: &str, look: &str) -> String {
  let subject = match kind {
    ReferenceKind::Look => format!("A character reference sheet for a music video: the same performer shown full-length three times side by side, from the front, the side and the back, standing on a plain white background, the whole figure in frame from head to shoes each time. The performer, {}: {}", name.trim(), sentence(description)),
    ReferenceKind::Location => format!("An establishing shot of a music video's location with nobody in it. The place, {}: {}", name.trim(), sentence(description)),
    ReferenceKind::Prop => format!("A reference photograph of a single prop from a music video, alone on a plain white background. The prop, {}: {}", name.trim(), sentence(description)),
  };
  format!("{subject} The video's look, which this picture must match: {} No text, captions or logos.", sentence(look))
}

fn still_prompt(design: &Design, image: &str, references: usize) -> String {
  let performer = match references {
    0 => format!("The performer: {}", sentence(&design.character)),
    1 => format!("The attached picture is the performer's reference sheet: keep their face, hair, build and clothes exactly as shown wherever they appear. For the record, they are: {}", sentence(&design.character)),
    _ => format!("The attached pictures are references. The first is the performer's reference sheet: keep their face, hair, build and clothes exactly as shown wherever they appear. The others are the location and the props: keep them as shown. For the record, the performer is: {}", sentence(&design.character)),
  };
  let venue = if design.venue.trim().is_empty() { String::new() } else { format!(" The venue: {}", sentence(&design.venue)) };
  format!("A single frame from a music video. {} {performer}{venue} The video's look, which this frame must match: {} No text, captions, subtitles, borders or logos.", sentence(image), sentence(&design.look))
}

fn clip_prompt(look: &str, motion: &str) -> String {
  format!("{} Keep the look of the opening frame: {} No text or captions appear.", sentence(motion), sentence(look))
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

/// Runs `call`, waiting (`first_wait`, then twice as long each time) and trying again when the
/// provider turns it away for quota: dozens of stills and clips in a row will meet a per-minute
/// limit sooner or later.
async fn with_quota_retries<T, F, Fut>(first_wait: Duration, mut call: F) -> anyhow::Result<T>
where
  F: FnMut() -> Fut,
  Fut: Future<Output = anyhow::Result<T>>,
{
  let mut wait = first_wait;
  for _ in 0..QUOTA_RETRIES {
    match call().await {
      Err(err) if is_quota(&err) => {
        info!("Provider quota reached; trying again in {}s", wait.as_secs());
        tokio::time::sleep(wait).await;
        wait *= 2;
      },
      result => return result,
    }
  }
  call().await
}

fn is_quota(err: &anyhow::Error) -> bool {
  let text = err.to_string();
  text.contains("error 429") || text.contains("RESOURCE_EXHAUSTED") || text.to_ascii_lowercase().contains("quota")
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
      engine.start_song(7, SongRequest::Write { idea: "the last bus home".into(), notes: String::new(), current: None, writer: Writer::Gemini }, tx, egui::Context::default());

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
      engine.start_song(1, SongRequest::Plan { brief: Brief::default(), style: Style::default(), slots, writer: Writer::Gemini }, tx, egui::Context::default());

      let updates = finished(&rx, 1).await;
      let planned = updates.iter().find_map(|u| if let SongUpdate::Planned(shots) = u { Some(shots) } else { None }).unwrap();
      assert_eq!(planned.len(), 2);
      assert!(planned[0].image.contains("wide shot"), "the skipped low-energy slot gets a stand-in");
      assert_eq!(planned[1].image, "A close-up of hands on a keyboard");
      assert!(updates.iter().any(|u| matches!(u, SongUpdate::Note(n) if n.contains("left out 1 of 2"))));
      let _ = std::fs::remove_dir_all(dir);
    }

    /// Reference images are drawn in the style's look, saved in the project and added to the
    /// library; the performer's is a three-view sheet.
    #[tokio::test(flavor = "multi_thread")]
    async fn reference_images_are_made_for_the_performer_and_the_place() {
      let (base_url, requests) = fake_vertex(vec![image_reply(), image_reply()]);
      let (engine, dir) = engine(base_url);
      let project = dir.join("project");
      let items = vec![ReferenceJob { index: 0, version: 1, kind: ReferenceKind::Look, name: "The Orchestrator".into(), description: "Wiry, cable hair".into() }, ReferenceJob { index: 1, version: 3, kind: ReferenceKind::Location, name: "Rail Vault".into(), description: "A brick tunnel".into() }];
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

    /// Stills are saved in the project's folder with the performer's picture sent as a reference;
    /// one that fails is reported without stopping the others.
    #[tokio::test(flavor = "multi_thread")]
    async fn stills_land_in_the_project_and_failures_dont_stop_the_rest() {
      let blocked = json!({ "candidates": [{ "finishReason": "IMAGE_SAFETY" }] }).to_string();
      let (base_url, requests) = fake_vertex(vec![image_reply(), blocked]);
      let (engine, dir) = engine(base_url);
      let project = dir.join("project");
      let portrait = save(&project, "portrait_v1.png", &tiny_png()).unwrap();
      let shots = vec![StillJob { index: 0, version: 1, image: "The performer under a strip light".into() }, StillJob { index: 1, version: 2, image: "A tunnel".into() }];
      let (tx, rx) = std::sync::mpsc::channel();
      engine.start_song(3, SongRequest::Stills { dir: project.clone(), design: Design { look: "black and white".into(), character: "a dancer".into(), venue: "a tunnel".into() }, references: vec![portrait], aspect: Aspect::Tall, pro: false, shots }, tx, egui::Context::default());

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
      assert!(prompt.contains("keep their face") && prompt.contains("The venue: a tunnel."));
      assert!(engine.library().list("image", false, 0, 40).0.is_empty(), "stills stay out of the library");
      let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stopped_run_makes_nothing_more() {
      let (base_url, requests) = fake_vertex(Vec::new());
      let (engine, dir) = engine(base_url);
      let run = SongRun { engine: &engine, run_id: 9, tx: &std::sync::mpsc::channel().0, ctx: &egui::Context::default(), cancel: Arc::new(AtomicBool::new(true)), batch: "b".into() };
      let shots = vec![StillJob { index: 0, version: 1, image: "x".into() }];
      run.stage(SongRequest::Stills { dir: dir.join("p"), design: Design::default(), references: Vec::new(), aspect: Aspect::Wide, pro: false, shots }).await.unwrap();
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
      let full = music_prompt(&draft, false, false);
      assert!(full.starts_with("Fast breakbeat, 140 BPM\nA complete song"));
      assert!(full.ends_with("[Verse]\nla"));
      let instrumental = music_prompt(&draft, true, true);
      assert!(instrumental.contains("A 30-second piece.") && instrumental.ends_with("Instrumental only, no vocals."));
      assert!(!instrumental.contains("la"));
    }

    #[test]
    fn stills_say_what_the_attached_pictures_are() {
      let design = Design { look: "grainy black and white".into(), character: "a dancer in a lanyard".into(), venue: String::new() };
      assert!(still_prompt(&design, "A tunnel", 1).contains("The attached picture is the performer's reference sheet"));
      assert!(still_prompt(&design, "A tunnel", 3).contains("The others are the location and the props"));
      let alone = still_prompt(&design, "A tunnel", 0);
      assert!(alone.contains("A tunnel. The performer: a dancer in a lanyard. The video's look"));
      assert_eq!(clip_prompt("grainy", "She runs"), "She runs. Keep the look of the opening frame: grainy. No text or captions appear.");
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
      assert!((engine.estimate_song(&SongEstimate::Stills { count: 10, pro: false }) - 10.0 * 1120.0 * 60.0 / 1e6).abs() < 1e-9);
      let clips = |tier, resolution: &str| engine.estimate_song(&SongEstimate::Clips { seconds: 100, tier, resolution: resolution.to_owned() });
      assert!((clips(VeoTier::Fast, "720p") - 8.0).abs() < 1e-9);
      assert!((clips(VeoTier::Lite, "720p") - 3.0).abs() < 1e-9);
      assert!((clips(VeoTier::Standard, "1080p") - 20.0).abs() < 1e-9);
      let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn quota_errors_are_retried_and_others_arent() {
      let calls = Mutex::new(0);
      let result = with_quota_retries(Duration::ZERO, || async {
        let mut n = calls.lock().unwrap();
        *n += 1;
        if *n < 3 {
          Err(anyhow::anyhow!("Vertex AI error 429 Too Many Requests: quota exceeded"))
        } else {
          Ok(*n)
        }
      })
      .await;
      assert_eq!(result.unwrap(), 3);
      let calls = Mutex::new(0);
      let result: anyhow::Result<()> = with_quota_retries(Duration::ZERO, || async {
        *calls.lock().unwrap() += 1;
        Err(anyhow::anyhow!("Vertex AI error 400 Bad Request: bad prompt"))
      })
      .await;
      assert!(result.is_err());
      assert_eq!(*calls.lock().unwrap(), 1);
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

  /// A one-thread HTTP server that answers with `replies` in order, recording each request line
  /// and body.
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
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).unwrap();
      }
    });
    (url, seen)
  }
}
