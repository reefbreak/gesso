//! A music video in progress: everything the stages have produced so far, saved as
//! `project.json` in the project's own folder (`~/Artcraft/local/songs/<id>/`) next to its song,
//! reference images, stills and clips, so the work can be picked up later.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use log::warn;
use serde::{Deserialize, Serialize};

use super::beats::BeatGrid;
use super::cut::{Aspect, CutShot, Fades, Format, Grade, ShotMedia};
use super::plan::{self, Pace, Section, Slot, SongLength};
use super::writer::{Brief, Design, Draft, PlannedShot, Read, Style, TimedLine};

/// Under the local library's folder.
pub const DIRECTORY: &str = "songs";
const FILENAME: &str = "project.json";
/// How many earlier versions of the lyrics a project keeps.
const LYRIC_VERSIONS_KEPT: usize = 40;
const ID_PREFIX: &str = "song_";

/// The workflow's stages, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
  #[default]
  Idea,
  Lyrics,
  Song,
  Analysis,
  Style,
  Brief,
  References,
  Scenes,
  Shots,
  Final,
}

impl Stage {
  pub const ALL: [Stage; 10] = [Stage::Idea, Stage::Lyrics, Stage::Song, Stage::Analysis, Stage::Style, Stage::Brief, Stage::References, Stage::Scenes, Stage::Shots, Stage::Final];

  pub fn label(self) -> &'static str {
    match self {
      Stage::Idea => "Idea",
      Stage::Lyrics => "Lyrics",
      Stage::Song => "Song",
      Stage::Analysis => "Music Analysis",
      Stage::Style => "Visual Style",
      Stage::Brief => "Creative Brief",
      Stage::References => "Reference Images",
      Stage::Scenes => "Scenes",
      Stage::Shots => "Shots",
      Stage::Final => "Final Video",
    }
  }

  pub fn number(self) -> usize {
    Stage::ALL.iter().position(|s| *s == self).unwrap_or(0) + 1
  }
}

/// The track the video is cut to.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Track {
  pub path: PathBuf,
  /// The model that made it; empty for a file the user brought.
  pub model: String,
  /// What the music model said it sang, when it said.
  pub sung: String,
}

/// What a reference image shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
  /// The performer, front, side and back.
  #[default]
  Look,
  /// The performer's face in close-up, drawn from the sheet: on the sheet the face is small, so
  /// close-ups have little to go on without it.
  Face,
  Location,
  Prop,
}

impl ReferenceKind {
  pub fn label(self) -> &'static str {
    match self {
      ReferenceKind::Look => "look",
      ReferenceKind::Face => "face",
      ReferenceKind::Location => "location",
      ReferenceKind::Prop => "prop",
    }
  }
}

/// A picture every scene is drawn from, so the performer, the place and the props stay the same.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reference {
  pub kind: ReferenceKind,
  pub name: String,
  pub description: String,
  pub image: Option<PathBuf>,
  /// Counts regenerations, so each one gets its own file name (the media cache keys on it).
  pub version: u32,
  pub error: Option<String>,
}

/// Which pass over the pictures and clips is being worked on. The scenes, their wording and
/// their timing are shared; each pass keeps its own stills and clips.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
  /// Cheap, fast models, for settling the direction.
  #[default]
  Draft,
  /// The best models, once the direction is locked: each still is redrawn from its draft.
  Final,
}

impl Quality {
  pub const ALL: [Quality; 2] = [Quality::Draft, Quality::Final];

  pub fn label(self) -> &'static str {
    match self {
      Quality::Draft => "Draft",
      Quality::Final => "Final",
    }
  }

  /// The file-name tag that keeps the passes' media apart.
  pub fn tag(self) -> &'static str {
    match self {
      Quality::Draft => "draft",
      Quality::Final => "final",
    }
  }
}

/// One pass's media for a scene.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Take {
  pub still: Option<PathBuf>,
  /// The clip as the video model made it.
  pub clip: Option<PathBuf>,
  /// The clip with the mouth redrawn to the vocal, for scenes the performer sings.
  pub synced: Option<PathBuf>,
  /// What each was made from, so that redrawing a still shows which clip, lip sync and final
  /// still no longer match it: the draft still a final still was redrawn from, the still the
  /// clip was filmed from, and the clip that was lip-synced. `None` for media made before this
  /// was kept, which is taken to be up to date.
  pub still_from: Option<PathBuf>,
  pub clip_from: Option<PathBuf>,
  pub synced_from: Option<PathBuf>,
  /// The ending frame, for scenes that ask for one, with its own lineage: the opening still it
  /// was drawn to match, the draft ending a final one was redrawn from, and the ending the clip
  /// was filmed to.
  pub end_still: Option<PathBuf>,
  pub end_from: Option<PathBuf>,
  pub end_still_from: Option<PathBuf>,
  pub clip_end_from: Option<PathBuf>,
}

impl Take {
  /// The clip the cut uses: lip-synced when there is one.
  pub fn best_clip(&self) -> Option<&PathBuf> {
    self.synced.as_ref().or(self.clip.as_ref())
  }
}

/// One scene of the video: a still, then the clip made from it, in each pass.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shot {
  pub slot: Slot,
  /// The opening frame, for the image model.
  pub image: String,
  /// What moves, for the video model.
  pub motion: String,
  /// The frame the shot must end on, for the image model (empty for none). The video model is
  /// given the opening frame alone and invents the rest, which goes wrong when the shot ends
  /// somewhere exact: a push-in from far away arrives at a face it made up. With an ending frame
  /// drawn from the references, it is given both ends and fills in the move between.
  #[serde(default)]
  pub ending: String,
  /// Leave the ending frame undrawn and let the video model end the shot where it likes.
  /// Drawing is on unless this is set.
  pub skip_ending: bool,
  /// The scene's feeling in a few words.
  pub beat: String,
  /// The performer sings this scene's line to the camera, so its clip is lip-synced.
  pub sing: bool,
  pub draft: Take,
  #[serde(rename = "final")]
  pub finished: Take,
  /// Count regenerations, so each one gets its own file name (the media cache keys on it).
  pub still_version: u32,
  pub clip_version: u32,
  /// Why the last still failed (shown on the storyboard).
  pub error: Option<String>,
  /// Why the last clip or lip sync failed (shown on the Shots stage only: it says nothing about
  /// the still).
  #[serde(default)]
  pub clip_error: Option<String>,
  /// Where the draft's media was kept before scenes had two passes; moved into `draft` on load.
  #[serde(rename = "still", skip_serializing)]
  old_still: Option<PathBuf>,
  #[serde(rename = "clip", skip_serializing)]
  old_clip: Option<PathBuf>,
}

impl Shot {
  pub fn take(&self, quality: Quality) -> &Take {
    match quality {
      Quality::Draft => &self.draft,
      Quality::Final => &self.finished,
    }
  }

  pub fn take_mut(&mut self, quality: Quality) -> &mut Take {
    match quality {
      Quality::Draft => &mut self.draft,
      Quality::Final => &mut self.finished,
    }
  }

  /// The still to show for a pass: its own, or the draft's while the final hasn't been drawn.
  pub fn still_for(&self, quality: Quality) -> Option<&PathBuf> {
    self.take(quality).still.as_ref().or(self.draft.still.as_ref())
  }

  /// Whether any pass has a still (or a clip).
  pub fn has_still(&self) -> bool {
    self.draft.still.is_some() || self.finished.still.is_some()
  }

  pub fn has_clip(&self) -> bool {
    self.draft.clip.is_some() || self.finished.clip.is_some()
  }

  /// A new still for a pass. The clip filmed from the old still stays, now out of date; so does
  /// a final still that was redrawn from the old draft.
  pub fn set_still(&mut self, quality: Quality, path: PathBuf) {
    let guide = self.draft.still.clone();
    if quality == Quality::Draft && self.finished.still.is_some() && self.finished.still_from.is_none() {
      // A final still from before this was kept was redrawn from the draft that is being replaced.
      self.finished.still_from.clone_from(&guide);
    }
    let take = self.take_mut(quality);
    if take.clip.is_some() && take.clip_from.is_none() {
      take.clip_from.clone_from(&take.still);
    }
    take.still = Some(path);
    take.still_from = if quality == Quality::Final { guide } else { None };
  }

  /// A new clip for a pass, filmed from its still as it is now. A lip sync of the old clip stays,
  /// out of date.
  pub fn set_clip(&mut self, quality: Quality, path: PathBuf) {
    let take = self.take_mut(quality);
    if take.synced.is_some() && take.synced_from.is_none() {
      take.synced_from.clone_from(&take.clip);
    }
    let ending = self.end_frame(quality).cloned();
    let take = self.take_mut(quality);
    take.clip = Some(path);
    take.clip_from.clone_from(&take.still);
    take.clip_end_from = ending;
  }

  /// Whether the scene asks for an ending frame: it has one described and drawing it hasn't
  /// been turned off for the scene.
  pub fn wants_ending(&self) -> bool {
    self.has_ending() && !self.skip_ending
  }

  /// Whether the scene's ending frame is described (it is drawn only if the scene asks).
  pub fn has_ending(&self) -> bool {
    !self.ending.trim().is_empty()
  }

  /// The ending frame a pass's clip is filmed to: none unless the scene asks for one and it has
  /// been drawn.
  pub fn end_frame(&self, quality: Quality) -> Option<&PathBuf> {
    self.take(quality).end_still.as_ref().filter(|_| self.wants_ending())
  }

  /// A new ending frame for a pass, drawn to match its opening still as that is now.
  pub fn set_end_still(&mut self, quality: Quality, path: PathBuf) {
    let guide = self.draft.end_still.clone();
    let take = self.take_mut(quality);
    take.end_still = Some(path);
    take.end_from.clone_from(&take.still);
    take.end_still_from = if quality == Quality::Final { guide } else { None };
  }

  /// Whether a pass's ending frame has been overtaken: its opening still was redrawn after it was
  /// matched to it, or (in the final pass) the draft ending it was redrawn from has changed.
  pub fn end_stale(&self, quality: Quality) -> bool {
    let take = self.take(quality);
    self.wants_ending() && take.end_still.is_some() && (take.end_from != take.still || (quality == Quality::Final && take.end_still_from != self.draft.end_still))
  }

  /// Whether a pass's ending frame needs (re)drawing. It waits for the opening still, which it is
  /// drawn to match, and in the final pass for a draft ending to redraw.
  pub fn end_due(&self, quality: Quality) -> bool {
    let take = self.take(quality);
    let drawable = take.still.is_some() && (quality == Quality::Draft || self.draft.end_still.is_some());
    self.wants_ending() && drawable && (take.end_still.is_none() || self.end_stale(quality))
  }

  /// Whether a pass has what a clip is filmed from: the opening still, and the ending frame when
  /// the scene asks for one.
  pub fn clip_ready(&self, quality: Quality) -> bool {
    let take = self.take(quality);
    take.still.is_some() && (!self.wants_ending() || take.end_still.is_some())
  }

  /// A new lip sync for a pass, of its clip as it is now.
  pub fn set_synced(&mut self, quality: Quality, path: PathBuf) {
    let take = self.take_mut(quality);
    take.synced = Some(path);
    take.synced_from.clone_from(&take.clip);
  }

  /// Whether the final still was redrawn from a draft still that has since been replaced.
  pub fn still_stale(&self) -> bool {
    self.finished.still.is_some() && self.finished.still_from.is_some() && self.finished.still_from != self.draft.still
  }

  /// Whether a pass's clip was filmed from a still that has since been replaced.
  pub fn clip_stale(&self, quality: Quality) -> bool {
    let take = self.take(quality);
    // Or it was filmed to an ending frame that has since been added, redrawn or dropped.
    take.clip.is_some() && take.clip_from.is_some() && (take.clip_from != take.still || take.clip_end_from.as_ref() != self.end_frame(quality))
  }

  /// Whether a pass's lip sync is of a clip that has since been replaced, or is itself out of date.
  pub fn sync_stale(&self, quality: Quality) -> bool {
    let take = self.take(quality);
    take.synced.is_some() && (self.clip_stale(quality) || (take.synced_from.is_some() && take.synced_from != take.clip))
  }

  /// Whether a pass's still needs (re)drawing: the final pass redraws drafts, so its still is also
  /// due when the draft it came from has changed.
  pub fn still_due(&self, quality: Quality) -> bool {
    self.opening_due(quality) || self.end_due(quality)
  }

  /// Whether a pass's opening still needs (re)drawing.
  pub fn opening_due(&self, quality: Quality) -> bool {
    self.take(quality).still.is_none() || (quality == Quality::Final && self.still_stale())
  }

  /// Whether a pass's clip needs (re)making: there is a still, and no clip filmed from it.
  pub fn clip_due(&self, quality: Quality) -> bool {
    self.clip_ready(quality) && (self.take(quality).clip.is_none() || self.clip_stale(quality))
  }

  /// Whether a sung scene's lip sync needs (re)doing: there is a clip, and no sync of it.
  pub fn sync_due(&self, quality: Quality) -> bool {
    let take = self.take(quality);
    self.sing && take.clip.is_some() && (take.synced.is_none() || self.sync_stale(quality))
  }
}

/// A music video in progress.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SongProject {
  /// What the video cost before its calls were counted one by one: an estimate from the files
  /// in its folder, part by part, made once (`None` until then).
  pub earlier_cost: Option<Vec<(String, f64)>>,
  pub id: String,
  pub created_at: String,
  pub updated_at: String,
  /// The stage the user last worked on.
  pub stage: Stage,
  pub idea: String,
  pub title: String,
  /// The music direction.
  pub style: String,
  pub lyrics: String,
  pub track: Option<Track>,
  pub grid: Option<BeatGrid>,
  pub sections: Vec<Section>,
  pub read: Read,
  pub lines: Vec<TimedLine>,
  /// The user's own note on how the video should look.
  pub direction: String,
  pub visual: Option<Style>,
  pub brief: Option<Brief>,
  pub references: Vec<Reference>,
  /// The pass being worked on.
  pub quality: Quality,
  /// The performer sings on camera: the writer marks scenes to sing, and their clips are
  /// lip-synced (needs a fal key).
  pub lip_sync: bool,
  /// The track's vocal stem, which lip sync follows better than the full mix.
  pub vocals: Option<PathBuf>,
  pub aspect: Aspect,
  pub grade: Grade,
  /// The film's fades from and to black.
  #[serde(default)]
  pub fades: Fades,
  /// How long a song to write for and ask Lyria for.
  #[serde(default)]
  pub song_length: SongLength,
  /// The shape the film is cut to (Instagram's, or as filmed).
  #[serde(default)]
  pub format: Format,
  pub pace: Pace,
  pub shots: Vec<Shot>,
  pub cut: Option<PathBuf>,
  pub cut_version: u32,
  /// Whether clip errors are kept apart from still errors (`Shot::clip_error`); projects saved
  /// before that are sorted out when they are opened.
  #[serde(default)]
  errors_split: bool,
  /// What `cut` was made from (`None` for a film cut before this was recorded).
  #[serde(default)]
  pub cut_of: Option<CutOf>,
  /// The video's files that are in the library already, so opening it adds only the rest.
  #[serde(default)]
  pub in_library: BTreeSet<PathBuf>,
  /// Earlier versions of the words, oldest first: kept whenever a rewrite, a restore or a new
  /// song is about to move on from them.
  #[serde(default)]
  pub lyric_versions: Vec<LyricVersion>,
}

/// The song on paper as it stood at some point.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricVersion {
  pub at: String,
  /// Why it was kept ("Before a rewrite").
  pub why: String,
  pub title: String,
  pub style: String,
  pub lyrics: String,
}

/// What the film on file was cut from: the pass, and each scene's clip or still in order. A cut
/// goes out of date without anyone seeing it, when clips are made after it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CutOf {
  pub quality: Quality,
  pub media: Vec<PathBuf>,
  /// The finish and fades it was cut with (`None` for a film cut before they were recorded,
  /// which isn't held against it).
  pub finish: Option<(Grade, Fades)>,
  /// The shape it was cut to (`None` for a film cut before that was recorded).
  pub format: Option<Format>,
}

/// What in a pass is waiting to be made or has been overtaken by a change upstream of it: the
/// video's lineage runs still, then clip, then lip sync, then film.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lineage {
  /// Final stills whose draft has been redrawn since (always none in the draft pass).
  pub stills_stale: usize,
  pub stills_missing: usize,
  pub clips_missing: usize,
  /// Clips filmed from a still that has been redrawn since.
  pub clips_stale: usize,
  pub syncs_missing: usize,
  /// Lip syncs of a clip that has been remade since.
  pub syncs_stale: usize,
  /// Whether the film is what a cut would make now (`None`: no film, or not known).
  pub film_current: Option<bool>,
}

impl Lineage {
  pub fn stills(&self) -> usize {
    self.stills_stale + self.stills_missing
  }

  pub fn clips(&self) -> usize {
    self.clips_missing + self.clips_stale
  }

  pub fn syncs(&self) -> usize {
    self.syncs_missing + self.syncs_stale
  }

  /// Whether everything downstream of the scenes is made and up to date.
  pub fn settled(&self) -> bool {
    self.stills() + self.clips() + self.syncs() == 0 && self.film_current != Some(false)
  }
}

/// What a cut would be made of: how many scenes come from each kind of source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CutSources {
  /// Clips of the pass being cut.
  pub own_clips: usize,
  /// Clips of the other pass, used where this pass has none.
  pub borrowed_clips: usize,
  /// Stills held on screen where a scene has no clip in either pass.
  pub stills: usize,
  pub missing: usize,
  /// Of this pass's own clips, those filmed from a still that has been redrawn since.
  pub stale_clips: usize,
}

/// A saved project, for the list on the first stage.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
  pub id: String,
  pub title: String,
  pub updated_at: String,
  pub stage: Stage,
  pub shots: usize,
  /// What the video cost before its calls were counted one by one, once that's been worked
  /// out; until then, `made` is what to work it out from.
  pub earlier_cost: Option<Vec<(String, f64)>>,
  pub made: Made,
}

/// What a video's folder holds that was paid for: every version of everything, since a redraw
/// keeps the picture it replaces. Used to price the work done before Gesso counted each video's
/// calls. Imported files cost nothing and aren't counted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Made {
  pub songs: usize,
  pub references: usize,
  /// Stills, clips and their seconds, and lip-synced seconds, for the draft pass then the final.
  pub stills: [usize; 2],
  pub clips: [usize; 2],
  pub clip_seconds: [u32; 2],
  pub synced_seconds: [f64; 2],
  /// Seconds of track the vocal was separated from (zero when it wasn't).
  pub separated_seconds: f64,
}

impl Made {
  pub fn is_empty(&self) -> bool {
    *self == Self::default()
  }
}

impl SongProject {
  pub fn new() -> Self {
    let now = chrono::Utc::now().to_rfc3339();
    Self { id: format!("{ID_PREFIX}{}", uuid::Uuid::new_v4().simple()), created_at: now.clone(), updated_at: now, errors_split: true, ..Default::default() }
  }

  /// A saved project by id.
  pub fn load(library_dir: &Path, id: &str) -> Option<Self> {
    let path = library_dir.join(DIRECTORY).join(id).join(FILENAME);
    let text = std::fs::read_to_string(&path).ok()?;
    let mut project: Self = serde_json::from_str(&text).map_err(|err| warn!("Ignoring unreadable {}: {err}", path.display())).ok()?;
    // Projects saved before scenes had two passes kept the draft's media on the scene itself.
    for shot in &mut project.shots {
      if let Some(still) = shot.old_still.take() {
        shot.draft.still.get_or_insert(still);
      }
      if let Some(clip) = shot.old_clip.take() {
        shot.draft.clip.get_or_insert(clip);
      }
      // One error used to cover stills and clips alike. On a scene that has its still, it came
      // from the clip or the lip sync, so it belongs to the Shots stage.
      if !project.errors_split && shot.has_still() && shot.clip_error.is_none() {
        shot.clip_error = shot.error.take();
      }
    }
    project.errors_split = true;
    project.ensure_face();
    project.place_lyrics();
    Some(project)
  }

  /// Every saved project, most recently worked on first.
  pub fn list(library_dir: &Path) -> Vec<Summary> {
    let Ok(entries) = std::fs::read_dir(library_dir.join(DIRECTORY)) else {
      return Vec::new();
    };
    let mut out: Vec<Summary> = entries.flatten().filter_map(|entry| Self::load(library_dir, &entry.file_name().to_string_lossy())).map(|p| Summary { title: p.display_title(), made: if p.earlier_cost.is_none() { p.made(&p.dir(library_dir)) } else { Made::default() }, earlier_cost: p.earlier_cost, id: p.id, updated_at: p.updated_at, stage: p.stage, shots: p.shots.len() }).collect();
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
  }

  /// What `dir`, the project's folder, holds that was paid for, read from the files' names
  /// (`shot_012_final_v3.png`, `clip_012_draft_synced_v2.mp4`).
  pub fn made(&self, dir: &Path) -> Made {
    let names = |folder: PathBuf| -> Vec<String> { std::fs::read_dir(folder).into_iter().flatten().flatten().filter(|entry| entry.path().is_file()).map(|entry| entry.file_name().to_string_lossy().to_lowercase()).filter(|name| !name.starts_with('.') && !name.ends_with(".thumb.jpg") && !name.contains("imported")).collect() };
    let mut made = Made::default();
    for name in names(dir.to_path_buf()) {
      if name.starts_with("song_v") {
        made.songs += 1;
      } else if name.starts_with("vocals.") {
        made.separated_seconds = self.grid.as_ref().map_or(0.0, |grid| grid.duration);
      }
    }
    made.references = names(dir.join("references")).iter().filter(|name| name.ends_with(".png") || name.ends_with(".jpg") || name.ends_with(".webp")).count();
    for name in names(dir.join("shots")) {
      if let Some(pass) = pass_of(&name).filter(|_| name.starts_with("shot_")) {
        made.stills[pass] += 1;
      }
    }
    for name in names(dir.join("clips")) {
      let Some(pass) = pass_of(&name).filter(|_| name.starts_with("clip_") && name.ends_with(".mp4")) else {
        continue;
      };
      // The scene's length as planned now; a clip of a scene since removed counts as the longest.
      let seconds = name.split('_').nth(1).and_then(|n| n.parse::<usize>().ok()).and_then(|n| self.shots.get(n.checked_sub(1)?)).map(|shot| shot.slot.seconds());
      if name.contains("_synced_") {
        made.synced_seconds[pass] += seconds.unwrap_or(8.0);
      } else {
        made.clips[pass] += 1;
        made.clip_seconds[pass] += u32::from(seconds.map_or(8, super::plan::clip_seconds));
      }
    }
    made
  }

  /// The project's folder, where its song, stills and clips are kept.
  pub fn dir(&self, library_dir: &Path) -> PathBuf {
    library_dir.join(DIRECTORY).join(&self.id)
  }

  /// Writes the project atomically, so a crash can't truncate it.
  pub fn save(&mut self, library_dir: &Path) -> anyhow::Result<()> {
    self.updated_at = chrono::Utc::now().to_rfc3339();
    let dir = self.dir(library_dir);
    std::fs::create_dir_all(&dir)?;
    let mut file = tempfile::NamedTempFile::new_in(&dir)?;
    file.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
    file.persist(dir.join(FILENAME))?;
    Ok(())
  }

  pub fn display_title(&self) -> String {
    let title = self.title.trim();
    if !title.is_empty() {
      return title.to_owned();
    }
    let idea: String = self.idea.trim().chars().take(48).collect();
    if idea.is_empty() {
      "Untitled".to_owned()
    } else {
      idea
    }
  }

  /// Keeps the words as they stand among the earlier versions, unless they're empty or the same
  /// as some version already kept. Only the newest [`LYRIC_VERSIONS_KEPT`] stay.
  pub fn keep_lyrics(&mut self, why: &str) {
    let same = |v: &LyricVersion| v.title == self.title && v.style == self.style && v.lyrics == self.lyrics;
    if self.lyrics.trim().is_empty() || self.lyric_versions.iter().any(same) {
      return;
    }
    self.lyric_versions.push(LyricVersion { at: chrono::Utc::now().to_rfc3339(), why: why.to_owned(), title: self.title.clone(), style: self.style.clone(), lyrics: self.lyrics.clone() });
    let extra = self.lyric_versions.len().saturating_sub(LYRIC_VERSIONS_KEPT);
    self.lyric_versions.drain(..extra);
  }

  /// Goes back to an earlier version of the words, keeping the present ones first.
  pub fn restore_lyrics(&mut self, index: usize) {
    let Some(version) = self.lyric_versions.get(index).cloned() else {
      return;
    };
    self.keep_lyrics("Before going back to an earlier version");
    self.title = version.title;
    self.style = version.style;
    self.lyrics = version.lyrics;
  }

  pub fn draft(&self) -> Draft {
    Draft { title: self.title.clone(), style: self.style.clone(), lyrics: self.lyrics.clone() }
  }

  /// What the image and video prompts carry from the style and the brief.
  pub fn design(&self) -> Design {
    let brief = self.brief.as_ref();
    Design { look: self.visual.as_ref().map(|s| s.block.clone()).unwrap_or_default(), character: brief.map(|b| b.character.description.clone()).unwrap_or_default(), venue: brief.map(|b| b.venue.description.clone()).unwrap_or_default() }
  }

  /// The furthest stage that has what it needs to be opened.
  pub fn furthest(&self) -> Stage {
    if self.lyrics.trim().is_empty() {
      Stage::Idea
    } else if self.track.is_none() {
      Stage::Song
    } else if self.grid.is_none() {
      Stage::Analysis
    } else if self.visual.is_none() {
      Stage::Style
    } else if self.brief.is_none() {
      Stage::Brief
    } else if self.shots.is_empty() || self.shots.iter().all(|s| !s.has_still()) {
      // Reference images help but aren't required: scenes can be drawn from words alone.
      Stage::Scenes
    } else if self.shots.iter().all(|s| !s.has_clip()) {
      Stage::Shots
    } else {
      Stage::Final
    }
  }

  /// Whether `stage` can be opened yet.
  pub fn reachable(&self, stage: Stage) -> bool {
    stage <= self.furthest()
  }

  /// A new brief: the reference list becomes its performer, venue and props. Pictures made for
  /// subjects whose description hasn't changed are kept.
  pub fn set_brief(&mut self, brief: Brief) {
    let wanted = [(ReferenceKind::Look, &brief.character)].into_iter().chain([(ReferenceKind::Location, &brief.venue)]).chain(brief.props.iter().map(|prop| (ReferenceKind::Prop, prop)));
    let references = wanted
      .filter(|(_, subject)| !subject.description.trim().is_empty())
      .map(|(kind, subject)| {
        let old = self.references.iter().find(|r| r.kind == kind && r.name == subject.name);
        let version = old.map_or(0, |r| r.version);
        let image = old.filter(|r| r.description == subject.description).and_then(|r| r.image.clone());
        Reference { kind, name: subject.name.clone(), description: subject.description.clone(), image, version, error: None }
      })
      .collect();
    // The head shot is of the sheet's performer: it stays only while the sheet does.
    let face = self.references.iter().find(|r| r.kind == ReferenceKind::Face).cloned();
    self.references = references;
    self.brief = Some(brief);
    self.keep_face(face.filter(|_| self.references.iter().any(|r| r.kind == ReferenceKind::Look && r.image.is_some())));
  }

  /// Gives the performer a head-shot reference beside their sheet: `kept` (the one there was) with
  /// the performer's current name, or an empty one to be drawn.
  fn keep_face(&mut self, kept: Option<Reference>) {
    self.references.retain(|r| r.kind != ReferenceKind::Face);
    let Some(sheet) = self.references.iter().position(|r| r.kind == ReferenceKind::Look) else {
      return;
    };
    let (name, description) = (self.references[sheet].name.clone(), self.references[sheet].description.clone());
    let face = match kept {
      Some(face) => Reference { name, ..face },
      None => Reference { kind: ReferenceKind::Face, name, description, image: None, version: 0, error: None },
    };
    self.references.insert(sheet + 1, face);
  }

  /// Adds the head-shot reference to a project saved before there was one.
  fn ensure_face(&mut self) {
    if !self.references.iter().any(|r| r.kind == ReferenceKind::Face) {
      self.keep_face(None);
    }
  }

  /// Puts each sung line on the scene it is sung over, leaving the scenes, their stills and their
  /// clips as they are: by the lines' timings when listening to the track gave any, else by the
  /// sections, when planning again would still cut the same scenes.
  pub fn place_lyrics(&mut self) {
    let mut slots: Vec<Slot> = self.shots.iter().map(|shot| shot.slot.clone()).collect();
    if self.lines.iter().any(|line| !line.text.trim().is_empty()) {
      plan::place_lines(&mut slots, &self.lines);
    } else if let Some(grid) = &self.grid {
      let fresh = plan::slots(grid, &self.sections, self.pace);
      if fresh.len() != slots.len() {
        return;
      }
      slots = fresh;
    } else {
      return;
    }
    for (shot, slot) in self.shots.iter_mut().zip(slots) {
      shot.slot.lyric = slot.lyric;
    }
  }

  /// After the brief was edited by hand: the references take its names and wording and keep their
  /// pictures, which can be drawn again on the next stage.
  pub fn follow_edited_brief(&mut self) {
    let Some(brief) = &self.brief else {
      return;
    };
    let wanted = [(ReferenceKind::Look, &brief.character)].into_iter().chain([(ReferenceKind::Location, &brief.venue)]).chain(brief.props.iter().map(|prop| (ReferenceKind::Prop, prop)));
    let mut old = std::mem::take(&mut self.references);
    let references = wanted
      .filter(|(_, subject)| !subject.description.trim().is_empty())
      .map(|(kind, subject)| {
        // The performer and the venue are one of a kind; a prop is found by whichever half wasn't edited.
        let found = old.iter().position(|r| r.kind == kind && (kind != ReferenceKind::Prop || r.name == subject.name || r.description == subject.description));
        let kept = found.map(|i| old.remove(i));
        Reference { kind, name: subject.name.clone(), description: subject.description.clone(), image: kept.as_ref().and_then(|r| r.image.clone()), version: kept.as_ref().map_or(0, |r| r.version), error: kept.and_then(|r| r.error) }
      })
      .collect();
    let face = old.into_iter().find(|r| r.kind == ReferenceKind::Face);
    self.references = references;
    self.keep_face(face);
  }

  /// After a reference's description was edited by hand: the brief takes the new wording (props
  /// are found by name).
  pub fn follow_edited_references(&mut self) {
    let Some(brief) = &mut self.brief else {
      return;
    };
    for reference in &self.references {
      let subject = match reference.kind {
        ReferenceKind::Look => Some(&mut brief.character),
        // The head shot's wording is its own: the brief describes the whole performer.
        ReferenceKind::Face => None,
        ReferenceKind::Location => Some(&mut brief.venue),
        ReferenceKind::Prop => brief.props.iter_mut().find(|prop| prop.name == reference.name),
      };
      if let Some(subject) = subject {
        subject.description.clone_from(&reference.description);
      }
    }
  }

  /// The reference pictures that exist, the performer first, each with what it shows: what every
  /// still is drawn from.
  pub fn reference_images(&self) -> Vec<(ReferenceKind, PathBuf)> {
    let mut made: Vec<&Reference> = self.references.iter().filter(|r| r.image.is_some()).collect();
    made.sort_by_key(|r| match r.kind {
      ReferenceKind::Look => 0,
      ReferenceKind::Face => 1,
      ReferenceKind::Location => 2,
      ReferenceKind::Prop => 3,
    });
    made.into_iter().filter_map(|r| r.image.clone().map(|image| (r.kind, image))).collect()
  }

  /// Replaces the storyboard with the writer's scenes for `slots`. Stills and clips made for the
  /// old storyboard no longer match, so they're dropped (their files stay on disk).
  pub fn set_plan(&mut self, slots: &[Slot], planned: &[PlannedShot]) {
    self.shots = slots
      .iter()
      .map(|slot| {
        let scene = planned.iter().find(|p| p.index == slot.index);
        let (still_version, clip_version) = self.shots.get(slot.index).map_or((0, 0), |old| (old.still_version, old.clip_version));
        Shot { slot: slot.clone(), image: scene.map(|p| p.image.clone()).unwrap_or_default(), motion: scene.map(|p| p.motion.clone()).unwrap_or_default(), beat: scene.map(|p| p.beat.clone()).unwrap_or_default(), ending: scene.map(|p| p.ending.clone()).unwrap_or_default(), sing: self.lip_sync && scene.is_some_and(|p| p.sing), still_version, clip_version, ..Default::default() }
      })
      .collect();
    self.cut = None;
  }

  /// The described scenes that have no ending frame described yet.
  pub fn scenes_without_ending(&self) -> Vec<usize> {
    self.shots.iter().enumerate().filter(|(_, shot)| !shot.image.trim().is_empty() && !shot.has_ending()).map(|(index, _)| index).collect()
  }

  /// Gives scenes the ending frames a writer described; a scene that already has one keeps it.
  pub fn set_endings(&mut self, endings: &[(usize, String)]) {
    for (index, ending) in endings {
      if let Some(shot) = self.shots.get_mut(*index).filter(|shot| !shot.has_ending()) {
        shot.ending = ending.trim().to_owned();
      }
    }
  }

  /// The cut's shots for a pass. Each slot takes that pass's clip (lip-synced when there is
  /// one), then the other pass's clip, then a still held for the slot. `None` while any slot has
  /// nothing at all.
  pub fn cut_shots(&self, quality: Quality) -> Option<Vec<CutShot>> {
    if self.shots.is_empty() {
      return None;
    }
    let other = if quality == Quality::Draft { Quality::Final } else { Quality::Draft };
    self
      .shots
      .iter()
      .map(|shot| {
        let clip = shot.take(quality).best_clip().or_else(|| shot.take(other).best_clip()).cloned().map(ShotMedia::Clip);
        let still = || shot.take(quality).still.as_ref().or(shot.take(other).still.as_ref()).cloned().map(ShotMedia::Still);
        clip.or_else(still).map(|media| CutShot { start: shot.slot.start, end: shot.slot.end, media })
      })
      .collect()
  }

  /// What in `quality` is waiting or out of date, stage by stage.
  pub fn lineage(&self, quality: Quality) -> Lineage {
    let mut lineage = Lineage { film_current: self.cut_is_current(quality), ..Default::default() };
    for shot in self.shots.iter().filter(|shot| !shot.image.trim().is_empty()) {
      let take = shot.take(quality);
      // The final pass redraws drafts, so a scene with no draft still has nothing waiting there.
      let drawable = quality == Quality::Draft || shot.draft.still.is_some();
      match (take.still.is_some(), quality == Quality::Final && shot.still_stale()) {
        (false, _) if drawable => lineage.stills_missing += 1,
        (true, true) => lineage.stills_stale += 1,
        _ => {},
      }
      // An ending frame is a still too, with its own place in the count.
      if shot.end_due(quality) {
        if take.end_still.is_some() {
          lineage.stills_stale += 1;
        } else {
          lineage.stills_missing += 1;
        }
      }
      if take.still.is_some() {
        match (take.clip.is_some(), shot.clip_stale(quality)) {
          (false, _) => lineage.clips_missing += 1,
          (true, true) => lineage.clips_stale += 1,
          _ => {},
        }
      }
      if self.lip_sync && shot.sing && take.clip.is_some() {
        match (take.synced.is_some(), shot.sync_stale(quality)) {
          (false, _) => lineage.syncs_missing += 1,
          (true, true) => lineage.syncs_stale += 1,
          _ => {},
        }
      }
    }
    lineage
  }

  /// The clip or still each scene would contribute to a cut in `quality`, in order.
  pub fn cut_media(&self, quality: Quality) -> Option<Vec<PathBuf>> {
    Some(self.cut_shots(quality)?.into_iter().map(|shot| match shot.media {
      ShotMedia::Clip(path) | ShotMedia::Still(path) => path,
    }).collect())
  }

  /// Whether the film on file is what a cut in `quality` would make now: `None` when there is no
  /// film, or it was cut before its sources were recorded.
  pub fn cut_is_current(&self, quality: Quality) -> Option<bool> {
    let made = self.cut.as_ref().and(self.cut_of.as_ref())?;
    let same_finish = made.finish.is_none_or(|finish| finish == (self.grade, self.fades)) && made.format.is_none_or(|format| format == self.format);
    Some(made.quality == quality && same_finish && Some(&made.media) == self.cut_media(quality).as_ref())
  }

  /// What a cut in `quality` would be made of, scene by scene: that pass's own clips, clips
  /// borrowed from the other pass, stills standing in for clips, and scenes with nothing at all.
  pub fn cut_sources(&self, quality: Quality) -> CutSources {
    let other = if quality == Quality::Draft { Quality::Final } else { Quality::Draft };
    let mut sources = CutSources::default();
    for shot in &self.shots {
      if shot.take(quality).best_clip().is_some() {
        sources.own_clips += 1;
        sources.stale_clips += usize::from(shot.clip_stale(quality));
      } else if shot.take(other).best_clip().is_some() {
        sources.borrowed_clips += 1;
      } else if shot.has_still() {
        sources.stills += 1;
      } else {
        sources.missing += 1;
      }
    }
    sources
  }

  /// Seconds of video a pass's clips would take to generate, at the clip lengths Veo makes (or
  /// `whole_clips_of` seconds each, for resolutions that only come in one length).
  pub fn seconds_to_generate(&self, quality: Quality, only_missing: bool, whole_clips_of: Option<u16>) -> u32 {
    self.shots.iter().filter(|s| s.clip_ready(quality) && (!only_missing || s.clip_due(quality))).map(|s| u32::from(whole_clips_of.unwrap_or_else(|| super::plan::clip_seconds(s.slot.seconds())))).sum()
  }

  /// The scenes of a pass that are sung and have a clip to sync, and how many seconds they run.
  pub fn scenes_to_sync(&self, quality: Quality, only_missing: bool) -> (Vec<usize>, f64) {
    let scenes: Vec<usize> = self.shots.iter().enumerate().filter(|(_, s)| s.sing && s.take(quality).clip.is_some() && (!only_missing || s.sync_due(quality))).map(|(i, _)| i).collect();
    let seconds = scenes.iter().map(|i| self.shots[*i].slot.seconds()).sum();
    (scenes, seconds)
  }
}

/// Which pass a file's name says it belongs to: 0 for the draft, 1 for the final.
fn pass_of(name: &str) -> Option<usize> {
  if name.contains("_draft") {
    Some(0)
  } else if name.contains("_final") {
    Some(1)
  } else {
    None
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::backend::local::song::plan::Energy;
  use crate::backend::local::song::writer::Subject;

  #[test]
  fn a_project_round_trips_and_lists_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = SongProject::new();
    first.idea = "a song about context windows".to_owned();
    first.save(dir.path()).unwrap();
    let mut second = SongProject::new();
    second.title = "Second".to_owned();
    second.shots = vec![Shot::default(), Shot::default()];
    std::thread::sleep(std::time::Duration::from_millis(5));
    second.save(dir.path()).unwrap();

    assert_eq!(SongProject::load(dir.path(), &first.id).unwrap(), first);
    let list = SongProject::list(dir.path());
    assert_eq!(list.iter().map(|s| (s.title.as_str(), s.shots)).collect::<Vec<_>>(), [("Second", 2), ("a song about context windows", 0)]);
    assert!(first.dir(dir.path()).join("project.json").exists());
    assert!(SongProject::list(&dir.path().join("missing")).is_empty());
  }

  #[test]
  fn stages_open_as_their_inputs_exist() {
    let mut project = SongProject::new();
    assert_eq!(project.furthest(), Stage::Idea);
    assert!(!project.reachable(Stage::Lyrics));
    project.lyrics = "[Verse]\na".to_owned();
    assert_eq!(project.furthest(), Stage::Song);
    assert!(project.reachable(Stage::Lyrics));
    project.track = Some(Track::default());
    assert_eq!(project.furthest(), Stage::Analysis);
    project.grid = Some(BeatGrid::steady(120.0, 30.0));
    assert_eq!(project.furthest(), Stage::Style);
    project.visual = Some(Style::default());
    assert_eq!(project.furthest(), Stage::Brief);
    project.set_brief(brief());
    assert_eq!(project.furthest(), Stage::Scenes);
    assert!(project.reachable(Stage::References));
    project.shots = vec![Shot { draft: Take { still: Some(PathBuf::from("a.png")), ..Default::default() }, ..Default::default() }];
    assert_eq!(project.furthest(), Stage::Shots);
    project.shots[0].draft.clip = Some(PathBuf::from("a.mp4"));
    assert_eq!(project.furthest(), Stage::Final);
    assert!(Stage::ALL.iter().all(|s| project.reachable(*s)));
  }

  #[test]
  fn a_brief_lists_its_references_and_keeps_pictures_that_still_fit() {
    let mut project = SongProject::new();
    project.visual = Some(Style { block: "grainy".into(), ..Default::default() });
    project.set_brief(brief());
    assert_eq!(project.references.iter().map(|r| (r.kind, r.name.as_str())).collect::<Vec<_>>(), [(ReferenceKind::Look, "The Orchestrator"), (ReferenceKind::Face, "The Orchestrator"), (ReferenceKind::Location, "Rail Vault"), (ReferenceKind::Prop, "Lanyard")]);
    for (i, reference) in project.references.iter_mut().enumerate() {
      reference.image = Some(PathBuf::from(format!("{i}.png")));
      reference.version = 2;
    }
    // Props first on disk order doesn't matter: the performer leads.
    project.references.reverse();
    assert_eq!(project.reference_images(), [(ReferenceKind::Look, PathBuf::from("0.png")), (ReferenceKind::Face, PathBuf::from("1.png")), (ReferenceKind::Location, PathBuf::from("2.png")), (ReferenceKind::Prop, PathBuf::from("3.png"))]);

    let mut revised = brief();
    revised.venue.description = "A flooded car park.".to_owned();
    project.set_brief(revised);
    assert_eq!(project.references[0].image, Some(PathBuf::from("0.png")), "the performer is unchanged");
    assert_eq!((project.references[1].kind, project.references[1].image.clone()), (ReferenceKind::Face, Some(PathBuf::from("1.png"))), "so their head shot stays");
    assert_eq!((project.references[2].image.clone(), project.references[2].version), (None, 2), "the venue changed, so its picture goes");

    assert_eq!(project.design(), Design { look: "grainy".into(), character: "Wiry, cable hair.".into(), venue: "A flooded car park.".into() });
    // A rewritten performer is a new person: the sheet goes, and the head shot with it.
    let mut recast = brief();
    recast.character.description = "Broad, shaven head.".to_owned();
    project.set_brief(recast);
    assert_eq!((project.references[0].image.clone(), project.references[1].kind, project.references[1].image.clone()), (None, ReferenceKind::Face, None));
  }

  #[test]
  fn an_edited_brief_renames_its_references_and_keeps_their_pictures() {
    let mut project = SongProject::new();
    project.set_brief(brief());
    for (i, reference) in project.references.iter_mut().enumerate() {
      reference.image = Some(PathBuf::from(format!("{i}.png")));
      reference.version = 2;
    }
    let edited = project.brief.as_mut().unwrap();
    edited.character.name = "The Captain".to_owned();
    edited.character.description = "Wiry, cable hair, a torn greatcoat.".to_owned();
    edited.props[0].description = "A brass compass with no needle.".to_owned();
    edited.props.push(Subject { name: "Ship's wheel".into(), role: String::new(), description: "A splintered wooden wheel.".into() });
    edited.props.push(Subject::default());
    project.follow_edited_brief();
    let summary: Vec<_> = project.references.iter().map(|r| (r.kind, r.name.as_str(), r.image.clone(), r.version)).collect();
    assert_eq!(summary, [(ReferenceKind::Look, "The Captain", Some(PathBuf::from("0.png")), 2), (ReferenceKind::Face, "The Captain", Some(PathBuf::from("1.png")), 2), (ReferenceKind::Location, "Rail Vault", Some(PathBuf::from("2.png")), 2), (ReferenceKind::Prop, "Lanyard", Some(PathBuf::from("3.png")), 2), (ReferenceKind::Prop, "Ship's wheel", None, 0)], "a prop with nothing written isn't a reference");
    assert_eq!(project.references[0].description, "Wiry, cable hair, a torn greatcoat.");
    assert_eq!(project.design().character, "Wiry, cable hair, a torn greatcoat.");
  }

  #[test]
  fn an_edited_reference_rewords_the_brief() {
    let mut project = SongProject::new();
    project.set_brief(brief());
    project.references[0].description = "Wiry, in a greatcoat.".to_owned();
    project.references[1].description = "The same face, colder.".to_owned();
    project.references[3].description = "A brass compass.".to_owned();
    project.follow_edited_references();
    let brief = project.brief.as_ref().unwrap();
    assert_eq!((brief.character.description.as_str(), brief.venue.description.as_str(), brief.props[0].description.as_str()), ("Wiry, in a greatcoat.", "A brick tunnel.", "A brass compass."));
  }

  /// A project saved when one error covered stills and clips: the error on a scene that has its
  /// still moves to the clip, and one on a scene without a still stays.
  #[test]
  fn old_clip_errors_leave_the_storyboard_when_a_project_is_opened() {
    let dir = std::env::temp_dir().join(format!("gesso-project-{}", uuid::Uuid::new_v4()));
    let mut project = SongProject::new();
    project.errors_split = false;
    project.shots = vec![Shot { draft: Take { still: Some(PathBuf::from("a.png")), ..Default::default() }, error: Some("Veo said no".into()), ..Default::default() }, Shot { error: Some("blocked".into()), ..Default::default() }];
    project.idea = "x".into();
    project.save(&dir).unwrap();
    let opened = SongProject::load(&dir, &project.id).unwrap();
    assert_eq!((opened.shots[0].error.as_deref(), opened.shots[0].clip_error.as_deref()), (None, Some("Veo said no")));
    assert_eq!((opened.shots[1].error.as_deref(), opened.shots[1].clip_error.as_deref()), (Some("blocked"), None));
    assert!(opened.errors_split);
    let _ = std::fs::remove_dir_all(dir);
  }

  /// A project saved before there was a head shot gains the empty card when it is opened, beside
  /// the sheet, with the other pictures untouched.
  #[test]
  fn an_older_project_gains_the_head_shot_card_when_opened() {
    let dir = std::env::temp_dir().join(format!("gesso-project-{}", uuid::Uuid::new_v4()));
    let mut project = SongProject::new();
    project.idea = "x".into();
    project.set_brief(brief());
    project.references.retain(|r| r.kind != ReferenceKind::Face);
    project.references[0].image = Some(PathBuf::from("sheet.png"));
    project.save(&dir).unwrap();
    let opened = SongProject::load(&dir, &project.id).unwrap();
    let cards: Vec<_> = opened.references.iter().map(|r| (r.kind, r.image.is_some())).collect();
    assert_eq!(cards, [(ReferenceKind::Look, true), (ReferenceKind::Face, false), (ReferenceKind::Location, false), (ReferenceKind::Prop, false)]);
    let _ = std::fs::remove_dir_all(dir);
  }

  /// A project whose sung lines sit a scene late is put right when it is opened, by the lines'
  /// timings, and keeps its stills.
  #[test]
  fn late_lyrics_are_moved_to_their_scenes_when_a_project_is_opened() {
    let dir = std::env::temp_dir().join(format!("gesso-project-{}", uuid::Uuid::new_v4()));
    let mut project = SongProject::new();
    project.idea = "x".into();
    let scene = |index: usize, start: f64, lyric: &str| Shot { slot: Slot { lyric: lyric.to_owned(), ..slot(index, start, start + 4.0) }, draft: Take { still: Some(PathBuf::from(format!("{index}.png"))), ..Default::default() }, ..Default::default() };
    project.shots = vec![scene(0, 0.0, ""), scene(1, 4.0, "one"), scene(2, 8.0, "two")];
    project.lines = vec![TimedLine { time: 1.0, text: "one".into() }, TimedLine { time: 5.0, text: "two".into() }];
    project.save(&dir).unwrap();
    let opened = SongProject::load(&dir, &project.id).unwrap();
    assert_eq!(opened.shots.iter().map(|s| s.slot.lyric.as_str()).collect::<Vec<_>>(), ["one", "two", ""]);
    assert!(opened.shots.iter().all(|s| s.draft.still.is_some()), "nothing but the words moves");
    let _ = std::fs::remove_dir_all(dir);
  }

  #[test]
  fn a_cut_says_what_it_is_made_of() {
    let mut project = SongProject::new();
    let clip = |path: &str| Some(PathBuf::from(path));
    project.shots = vec![
      Shot { draft: Take { clip: clip("d1.mp4"), ..Default::default() }, finished: Take { clip: clip("f1.mp4"), ..Default::default() }, ..Default::default() },
      Shot { draft: Take { clip: clip("d2.mp4"), ..Default::default() }, ..Default::default() },
      Shot { draft: Take { still: Some(PathBuf::from("d3.png")), ..Default::default() }, ..Default::default() },
      Shot::default(),
    ];
    assert_eq!(project.cut_sources(Quality::Final), CutSources { own_clips: 1, borrowed_clips: 1, stills: 1, missing: 1, stale_clips: 0 });
    assert_eq!(project.cut_sources(Quality::Draft), CutSources { own_clips: 2, borrowed_clips: 0, stills: 1, missing: 1, stale_clips: 0 });
  }

  /// A film cut from draft clips is out of date once final clips exist, or when the pass changes.
  #[test]
  fn a_cut_goes_out_of_date_when_its_clips_change() {
    let mut project = SongProject::new();
    project.shots = vec![Shot { slot: slot(0, 0.0, 4.0), draft: Take { clip: Some(PathBuf::from("d1.mp4")), ..Default::default() }, ..Default::default() }];
    assert_eq!(project.cut_is_current(Quality::Draft), None, "nothing cut yet");
    project.cut = Some(PathBuf::from("cut_v1.mp4"));
    assert_eq!(project.cut_is_current(Quality::Draft), None, "a film from before its sources were recorded");
    project.cut_of = Some(CutOf { quality: Quality::Draft, media: project.cut_media(Quality::Draft).unwrap(), finish: Some((project.grade, project.fades)), format: Some(project.format) });
    assert_eq!(project.cut_is_current(Quality::Draft), Some(true));
    project.format = Format::Feed;
    assert_eq!(project.cut_is_current(Quality::Draft), Some(false), "the shape was changed since");
    project.format = Format::AsFilmed;
    project.fades.fade_out = 2.0;
    assert_eq!(project.cut_is_current(Quality::Draft), Some(false), "the fades were changed since");
    project.fades = Fades::default();
    assert_eq!(project.cut_is_current(Quality::Final), Some(false), "it was cut in the other pass");
    project.shots[0].draft.synced = Some(PathBuf::from("d1_synced.mp4"));
    assert_eq!(project.cut_is_current(Quality::Draft), Some(false), "a lip sync has been made since");
  }

  /// Redrawing a still leaves its clip and lip sync in place but out of date, and remaking each
  /// in turn brings the scene back up to date.
  #[test]
  fn a_redrawn_still_puts_what_was_made_from_it_out_of_date() {
    let path = |name: &str| PathBuf::from(name);
    let mut shot = Shot { image: "A tunnel".into(), sing: true, ..Default::default() };
    shot.set_still(Quality::Draft, path("s1.png"));
    shot.set_clip(Quality::Draft, path("c1.mp4"));
    shot.set_synced(Quality::Draft, path("y1.mp4"));
    assert!(!shot.clip_stale(Quality::Draft) && !shot.sync_stale(Quality::Draft) && !shot.clip_due(Quality::Draft) && !shot.sync_due(Quality::Draft));

    shot.set_still(Quality::Draft, path("s2.png"));
    assert_eq!((shot.draft.clip.clone(), shot.draft.synced.clone()), (Some(path("c1.mp4")), Some(path("y1.mp4"))), "the old clip and sync are kept, not thrown away");
    assert!(shot.clip_stale(Quality::Draft) && shot.sync_stale(Quality::Draft) && shot.clip_due(Quality::Draft) && shot.sync_due(Quality::Draft));

    shot.set_clip(Quality::Draft, path("c2.mp4"));
    assert!(!shot.clip_stale(Quality::Draft) && shot.sync_stale(Quality::Draft), "the sync is of the old clip");
    shot.set_synced(Quality::Draft, path("y2.mp4"));
    assert!(!shot.sync_stale(Quality::Draft) && !shot.sync_due(Quality::Draft));
  }

  /// A final still follows the draft it was redrawn from, and media from before lineage was kept
  /// counts as up to date until something upstream of it changes.
  #[test]
  fn final_stills_follow_their_draft_and_older_media_is_taken_as_current() {
    let path = |name: &str| PathBuf::from(name);
    let mut shot = Shot { image: "A tunnel".into(), ..Default::default() };
    shot.set_still(Quality::Draft, path("d1.png"));
    shot.set_still(Quality::Final, path("f1.png"));
    assert!(!shot.still_stale() && !shot.still_due(Quality::Final));
    shot.set_still(Quality::Draft, path("d2.png"));
    assert!(shot.still_stale() && shot.still_due(Quality::Final), "its draft was redrawn");
    shot.set_still(Quality::Final, path("f2.png"));
    assert!(!shot.still_stale());

    let mut older = Shot { image: "A tunnel".into(), draft: Take { still: Some(path("s.png")), clip: Some(path("c.mp4")), synced: Some(path("y.mp4")), ..Default::default() }, finished: Take { still: Some(path("f.png")), ..Default::default() }, ..Default::default() };
    assert!(!older.clip_stale(Quality::Draft) && !older.sync_stale(Quality::Draft) && !older.still_stale());
    older.set_still(Quality::Draft, path("s2.png"));
    assert!(older.clip_stale(Quality::Draft) && older.sync_stale(Quality::Draft) && older.still_stale(), "the change upstream is noticed even so");
  }

  /// A scene with an ending frame: the ending waits for its opening still, the clip waits for
  /// both, and a change to either end puts the clip out of date.
  #[test]
  fn an_ending_frame_takes_its_place_in_the_lineage() {
    let path = |name: &str| PathBuf::from(name);
    let mut shot = Shot { image: "A dock, the performer far off".into(), ending: "The performer's face fills the frame".into(), ..Default::default() };
    assert!(shot.wants_ending() && !shot.end_due(Quality::Draft), "the ending is drawn to match the opening, so it waits for it");
    shot.set_still(Quality::Draft, path("open1.png"));
    assert!(shot.end_due(Quality::Draft) && shot.still_due(Quality::Draft) && !shot.opening_due(Quality::Draft));
    assert!(!shot.clip_ready(Quality::Draft) && !shot.clip_due(Quality::Draft), "no clip until both ends exist");
    shot.set_end_still(Quality::Draft, path("end1.png"));
    assert!(!shot.end_due(Quality::Draft) && shot.clip_ready(Quality::Draft) && shot.clip_due(Quality::Draft));
    assert_eq!(shot.end_frame(Quality::Draft), Some(&path("end1.png")));
    shot.set_clip(Quality::Draft, path("c1.mp4"));
    assert!(!shot.clip_stale(Quality::Draft) && !shot.clip_due(Quality::Draft));

    // Redrawing the ending overtakes the clip; redrawing the opening overtakes the ending too.
    shot.set_end_still(Quality::Draft, path("end2.png"));
    assert!(shot.clip_stale(Quality::Draft));
    shot.set_clip(Quality::Draft, path("c2.mp4"));
    shot.set_still(Quality::Draft, path("open2.png"));
    assert!(shot.end_stale(Quality::Draft) && shot.end_due(Quality::Draft) && shot.clip_stale(Quality::Draft));
    shot.set_end_still(Quality::Draft, path("end3.png"));
    shot.set_clip(Quality::Draft, path("c3.mp4"));
    assert!(!shot.end_stale(Quality::Draft) && !shot.clip_stale(Quality::Draft));

    // Dropping the ending leaves a clip that was filmed to one out of date, and nothing to draw.
    shot.ending.clear();
    assert!(shot.end_frame(Quality::Draft).is_none() && !shot.end_due(Quality::Draft) && shot.clip_stale(Quality::Draft) && shot.clip_ready(Quality::Draft));
  }

  /// The final pass redraws the draft's ending, after its own opening still.
  #[test]
  fn a_final_ending_follows_its_draft_and_its_own_opening() {
    let path = |name: &str| PathBuf::from(name);
    let mut shot = Shot { image: "A dock".into(), ending: "A face".into(), ..Default::default() };
    shot.set_still(Quality::Draft, path("d_open.png"));
    shot.set_still(Quality::Final, path("f_open.png"));
    assert!(!shot.end_due(Quality::Final), "there is no draft ending to redraw yet");
    shot.set_end_still(Quality::Draft, path("d_end.png"));
    assert!(shot.end_due(Quality::Final));
    shot.set_end_still(Quality::Final, path("f_end.png"));
    assert!(!shot.end_due(Quality::Final) && !shot.end_stale(Quality::Final));
    shot.set_end_still(Quality::Draft, path("d_end2.png"));
    assert!(shot.end_stale(Quality::Final), "its draft was redrawn");

    let mut project = SongProject::new();
    project.shots = vec![shot];
    let lineage = project.lineage(Quality::Final);
    assert_eq!((lineage.stills_stale, lineage.stills_missing), (1, 0), "the final ending counts among the stills to redraw");
  }

  #[test]
  fn the_lineage_counts_what_each_stage_has_to_do() {
    let path = |name: &str| PathBuf::from(name);
    let mut project = SongProject::new();
    project.lip_sync = true;
    let scene = |sing: bool| Shot { image: "A tunnel".into(), sing, ..Default::default() };
    project.shots = vec![scene(true), scene(false), scene(false), Shot::default()];
    assert_eq!(project.lineage(Quality::Draft), Lineage { stills_missing: 3, ..Default::default() }, "a scene with nothing written isn't waiting to be drawn");
    for (i, shot) in project.shots.iter_mut().take(2).enumerate() {
      shot.set_still(Quality::Draft, path(&format!("s{i}.png")));
      shot.set_clip(Quality::Draft, path(&format!("c{i}.mp4")));
    }
    project.shots[0].set_synced(Quality::Draft, path("y0.mp4"));
    assert_eq!(project.lineage(Quality::Draft), Lineage { stills_missing: 1, ..Default::default() });
    project.shots[0].set_still(Quality::Draft, path("s0b.png"));
    let lineage = project.lineage(Quality::Draft);
    assert_eq!((lineage.clips_stale, lineage.syncs_stale, lineage.clips(), lineage.settled()), (1, 1, 1, false));
    assert_eq!(project.cut_sources(Quality::Draft).stale_clips, 1);
    assert_eq!(project.seconds_to_generate(Quality::Draft, true, None), 4, "only the out-of-date clip is priced");
    assert_eq!(project.scenes_to_sync(Quality::Draft, true).0, [0]);
    // The final pass has nothing of its own yet: two drafts to redraw.
    assert_eq!(project.lineage(Quality::Final), Lineage { stills_missing: 2, ..Default::default() });
  }

  /// A video made before its calls were counted is priced from its folder: every version of
  /// every paid-for file, read from the names; imported files and small copies don't count.
  #[test]
  fn what_a_video_has_made_is_read_from_its_folder() {
    let dir = std::env::temp_dir().join(format!("gesso-made-{}", uuid::Uuid::new_v4()));
    for (folder, names) in [("", vec!["song_v1.mp3", "song_v2.mp3", "song_imported.mp3", "vocals.mp3", "cut_v1.mp4", "project.json"]), ("references", vec!["look_01_v1.png", "look_01_v2.png", "look_01_v2.thumb.jpg", "location_03_v1_imported.png"]), ("shots", vec!["shot_001_draft_v1.png", "shot_001_draft_v2.png", "shot_001_draft_end_v1.png", "shot_001_final_v1.png", "shot_001_draft_v1.thumb.jpg"]), ("clips", vec!["clip_001_draft_v1.mp4", "clip_002_draft_v1.mp4", "clip_009_final_v1.mp4", "clip_001_draft_synced_v1.mp4"])] {
      std::fs::create_dir_all(dir.join(folder)).unwrap();
      for name in names {
        std::fs::write(dir.join(folder).join(name), b"x").unwrap();
      }
    }
    let mut project = SongProject::new();
    project.shots = vec![Shot { slot: slot(0, 0.0, 3.5), ..Default::default() }, Shot { slot: slot(1, 3.5, 9.0), ..Default::default() }];
    let made = project.made(&dir);
    assert_eq!(made, Made { songs: 2, references: 2, stills: [3, 1], clips: [2, 1], clip_seconds: [4 + 6, 8], synced_seconds: [3.5, 0.0], separated_seconds: 0.0 });
    assert!(!made.is_empty() && SongProject::new().made(&dir.join("nowhere")).is_empty());
    let _ = std::fs::remove_dir_all(dir);
  }

  #[test]
  fn earlier_lyrics_are_kept_and_can_be_restored() {
    let mut project = SongProject::new();
    project.keep_lyrics("Before a rewrite");
    assert!(project.lyric_versions.is_empty(), "nothing written, nothing kept");
    project.lyrics = "first".into();
    project.keep_lyrics("Before a rewrite");
    project.keep_lyrics("Before a rewrite");
    project.lyrics = "second".into();
    assert_eq!(project.lyric_versions.iter().map(|v| v.lyrics.as_str()).collect::<Vec<_>>(), ["first"], "the same words are kept once");
    // Going back keeps what was there, so nothing is lost either way.
    project.restore_lyrics(0);
    assert_eq!(project.lyrics, "first");
    assert_eq!(project.lyric_versions.iter().map(|v| (v.lyrics.as_str(), v.why.as_str())).collect::<Vec<_>>(), [("first", "Before a rewrite"), ("second", "Before going back to an earlier version")]);
    for n in 0..60 {
      project.lyrics = format!("take {n}");
      project.keep_lyrics("Before a rewrite");
    }
    assert_eq!((project.lyric_versions.len(), project.lyric_versions.last().map(|v| v.lyrics.clone())), (LYRIC_VERSIONS_KEPT, Some("take 59".into())));
  }

  #[test]
  fn scenes_without_an_ending_frame_are_given_one() {
    let mut project = SongProject::new();
    project.shots = vec![Shot { image: "A dock".into(), ..Default::default() }, Shot { image: "A hull".into(), ending: "Mine".into(), ..Default::default() }, Shot::default()];
    assert_eq!(project.scenes_without_ending(), [0], "a scene with nothing described has nothing to end");
    project.set_endings(&[(0, " The dock, closer ".into()), (1, "Theirs".into()), (9, "nowhere".into())]);
    assert_eq!((project.shots[0].ending.as_str(), project.shots[1].ending.as_str()), ("The dock, closer", "Mine"), "an ending already written is kept");
    assert!(project.scenes_without_ending().is_empty());
    // Drawing it is on for every scene and can be turned off for one, which keeps its words.
    assert!(project.shots[0].wants_ending());
    project.shots[0].skip_ending = true;
    assert!(!project.shots[0].wants_ending() && project.shots[0].has_ending() && project.scenes_without_ending().is_empty());
  }

  #[test]
  fn a_new_plan_keeps_version_counters_and_drops_old_media() {
    let mut project = SongProject::new();
    project.shots = vec![Shot { draft: Take { still: Some(PathBuf::from("old.png")), clip: Some(PathBuf::from("old.mp4")), synced: None, ..Default::default() }, still_version: 3, clip_version: 2, ..Default::default() }];
    project.cut = Some(PathBuf::from("cut.mp4"));
    let slots = [slot(0, 0.0, 2.0), slot(1, 2.0, 4.0)];
    let scene = PlannedShot { index: 1, image: "B".into(), motion: "m".into(), beat: "manic".into(), sing: true, ending: "B, closer".into() };
    project.set_plan(&slots, std::slice::from_ref(&scene));
    assert_eq!(project.shots.len(), 2);
    assert_eq!((project.shots[0].draft.clone(), project.shots[0].still_version, project.shots[0].clip_version), (Take::default(), 3, 2));
    assert_eq!((project.shots[1].image.as_str(), project.shots[1].motion.as_str(), project.shots[1].beat.as_str()), ("B", "m", "manic"));
    assert!(!project.shots[1].sing, "nobody sings unless lip sync is on");
    assert_eq!(project.cut, None);
    project.lip_sync = true;
    project.set_plan(&slots, &[scene]);
    assert!(project.shots[1].sing && !project.shots[0].sing);
  }

  #[test]
  fn the_cut_uses_stills_where_clips_are_missing() {
    let mut project = SongProject::new();
    assert_eq!(project.cut_shots(Quality::Draft), None);
    let take = |still: &str, clip: Option<&str>| Take { still: Some(still.into()), clip: clip.map(Into::into), synced: None, ..Default::default() };
    project.shots = vec![Shot { slot: slot(0, 0.0, 2.0), draft: take("a.png", Some("a.mp4")), ..Default::default() }, Shot { slot: slot(1, 2.0, 7.0), draft: take("b.png", None), ..Default::default() }];
    let cut = project.cut_shots(Quality::Draft).unwrap();
    assert_eq!(cut[0].media, ShotMedia::Clip("a.mp4".into()));
    assert_eq!(cut[1], CutShot { start: 2.0, end: 7.0, media: ShotMedia::Still("b.png".into()) });
    // 2 s needs a 4-second clip, 5 s a 6-second one; only the second is missing.
    let seconds = |only_missing, whole| project.seconds_to_generate(Quality::Draft, only_missing, whole);
    assert_eq!((seconds(false, None), seconds(true, None), seconds(true, Some(8))), (10, 6, 8));
    project.shots.push(Shot::default());
    assert_eq!(project.cut_shots(Quality::Draft), None);
  }

  mod passes {
    use super::*;

    /// The final cut takes final clips where they exist and falls back to the draft's, so a
    /// half-finished final pass still cuts; a lip-synced clip always wins over its raw one.
    #[test]
    fn each_pass_cuts_from_its_own_media_first() {
      let mut project = SongProject::new();
      let draft = Take { still: Some("d.png".into()), clip: Some("d.mp4".into()), synced: Some("d_sync.mp4".into()), ..Default::default() };
      project.shots = vec![Shot { slot: slot(0, 0.0, 2.0), draft: draft.clone(), finished: Take { still: Some("f.png".into()), clip: Some("f.mp4".into()), synced: None, ..Default::default() }, ..Default::default() }, Shot { slot: slot(1, 2.0, 4.0), draft, finished: Take { still: Some("f2.png".into()), ..Default::default() }, ..Default::default() }];
      let media = |quality| project.cut_shots(quality).unwrap().into_iter().map(|s| s.media).collect::<Vec<_>>();
      assert_eq!(media(Quality::Final), [ShotMedia::Clip("f.mp4".into()), ShotMedia::Clip("d_sync.mp4".into())]);
      assert_eq!(media(Quality::Draft), [ShotMedia::Clip("d_sync.mp4".into()), ShotMedia::Clip("d_sync.mp4".into())]);
      assert_eq!(project.shots[1].still_for(Quality::Final), Some(&PathBuf::from("f2.png")));
      assert_eq!(Shot { draft: Take { still: Some("d.png".into()), ..Default::default() }, ..Default::default() }.still_for(Quality::Final), Some(&PathBuf::from("d.png")));
      // Final clips are made from final stills: one is drawn, none has a clip yet.
      assert_eq!(project.seconds_to_generate(Quality::Final, true, None), 4);
    }

    #[test]
    fn only_sung_scenes_with_a_clip_are_synced() {
      let mut project = SongProject::new();
      let clip = |synced: bool| Take { still: Some("s.png".into()), clip: Some("c.mp4".into()), synced: synced.then(|| "x.mp4".into()), ..Default::default() };
      project.shots = vec![Shot { slot: slot(0, 0.0, 2.0), sing: true, draft: clip(false), ..Default::default() }, Shot { slot: slot(1, 2.0, 5.0), sing: true, draft: clip(true), ..Default::default() }, Shot { slot: slot(2, 5.0, 9.0), draft: clip(false), ..Default::default() }, Shot { slot: slot(3, 9.0, 11.0), sing: true, ..Default::default() }];
      assert_eq!(project.scenes_to_sync(Quality::Draft, true), (vec![0], 2.0));
      assert_eq!(project.scenes_to_sync(Quality::Draft, false), (vec![0, 1], 5.0));
      assert_eq!(project.scenes_to_sync(Quality::Final, false), (Vec::new(), 0.0));
    }

    /// A project saved before scenes had two passes opens with its media in the draft.
    #[test]
    fn older_projects_keep_their_stills_and_clips() {
      let dir = tempfile::tempdir().unwrap();
      let folder = dir.path().join(DIRECTORY).join("song_old");
      std::fs::create_dir_all(&folder).unwrap();
      std::fs::write(folder.join(FILENAME), r#"{"id":"song_old","shots":[{"image":"A","still":"/p/a.png","clip":"/p/a.mp4","still_version":2}]}"#).unwrap();
      let mut project = SongProject::load(dir.path(), "song_old").unwrap();
      assert_eq!(project.shots[0].draft, Take { still: Some("/p/a.png".into()), clip: Some("/p/a.mp4".into()), synced: None, ..Default::default() });
      assert_eq!((project.quality, project.shots[0].still_version), (Quality::Draft, 2));
      project.save(dir.path()).unwrap();
      let saved = std::fs::read_to_string(folder.join(FILENAME)).unwrap();
      assert!(saved.contains("\"draft\"") && !saved.contains("\"still\": \"/p/a.png\",\n      \"still_version"), "{saved}");
      assert_eq!(SongProject::load(dir.path(), "song_old").unwrap().shots[0].draft, project.shots[0].draft);
    }
  }

  fn brief() -> Brief {
    let subject = |name: &str, description: &str| Subject { name: name.to_owned(), role: String::new(), description: description.to_owned() };
    Brief { title: "Runtime".into(), character: subject("The Orchestrator", "Wiry, cable hair."), venue: subject("Rail Vault", "A brick tunnel."), props: vec![subject("Lanyard", "A cracked badge."), subject("Empty", " ")], ..Default::default() }
  }

  fn slot(index: usize, start: f64, end: f64) -> Slot {
    Slot { index, start, end, section: "Verse".to_owned(), energy: Energy::Medium, lyric: String::new() }
  }
}
