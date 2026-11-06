//! A music video in progress: everything the stages have produced so far, saved as
//! `project.json` in the project's own folder (`~/Artcraft/local/songs/<id>/`) next to its song,
//! reference images, stills and clips, so the work can be picked up later.

use std::io::Write;
use std::path::{Path, PathBuf};

use log::warn;
use serde::{Deserialize, Serialize};

use super::beats::BeatGrid;
use super::cut::{Aspect, CutShot, Grade, ShotMedia};
use super::plan::{Pace, Section, Slot};
use super::writer::{Brief, Design, Draft, PlannedShot, Read, Style, TimedLine};

/// Under the local library's folder.
pub const DIRECTORY: &str = "songs";
const FILENAME: &str = "project.json";
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
  Location,
  Prop,
}

impl ReferenceKind {
  pub fn label(self) -> &'static str {
    match self {
      ReferenceKind::Look => "look",
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
}

/// A music video in progress.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SongProject {
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
  pub pace: Pace,
  pub shots: Vec<Shot>,
  pub cut: Option<PathBuf>,
  pub cut_version: u32,
  /// Whether clip errors are kept apart from still errors (`Shot::clip_error`); projects saved
  /// before that are sorted out when they are opened.
  #[serde(default)]
  errors_split: bool,
}

/// A saved project, for the list on the first stage.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
  pub id: String,
  pub title: String,
  pub updated_at: String,
  pub stage: Stage,
  pub shots: usize,
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
    Some(project)
  }

  /// Every saved project, most recently worked on first.
  pub fn list(library_dir: &Path) -> Vec<Summary> {
    let Ok(entries) = std::fs::read_dir(library_dir.join(DIRECTORY)) else {
      return Vec::new();
    };
    let mut out: Vec<Summary> = entries.flatten().filter_map(|entry| Self::load(library_dir, &entry.file_name().to_string_lossy())).map(|p| Summary { title: p.display_title(), id: p.id, updated_at: p.updated_at, stage: p.stage, shots: p.shots.len() }).collect();
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
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
    self.references = references;
    self.brief = Some(brief);
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
    self.references = references;
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
      ReferenceKind::Location => 1,
      ReferenceKind::Prop => 2,
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
        Shot { slot: slot.clone(), image: scene.map(|p| p.image.clone()).unwrap_or_default(), motion: scene.map(|p| p.motion.clone()).unwrap_or_default(), beat: scene.map(|p| p.beat.clone()).unwrap_or_default(), sing: self.lip_sync && scene.is_some_and(|p| p.sing), still_version, clip_version, ..Default::default() }
      })
      .collect();
    self.cut = None;
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

  /// Seconds of video a pass's clips would take to generate, at the clip lengths Veo makes (or
  /// `whole_clips_of` seconds each, for resolutions that only come in one length).
  pub fn seconds_to_generate(&self, quality: Quality, only_missing: bool, whole_clips_of: Option<u16>) -> u32 {
    self.shots.iter().filter(|s| s.take(quality).still.is_some() && (!only_missing || s.take(quality).clip.is_none())).map(|s| u32::from(whole_clips_of.unwrap_or_else(|| super::plan::clip_seconds(s.slot.seconds())))).sum()
  }

  /// The scenes of a pass that are sung and have a clip to sync, and how many seconds they run.
  pub fn scenes_to_sync(&self, quality: Quality, only_missing: bool) -> (Vec<usize>, f64) {
    let scenes: Vec<usize> = self.shots.iter().enumerate().filter(|(_, s)| s.sing && s.take(quality).clip.is_some() && (!only_missing || s.take(quality).synced.is_none())).map(|(i, _)| i).collect();
    let seconds = scenes.iter().map(|i| self.shots[*i].slot.seconds()).sum();
    (scenes, seconds)
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
    assert_eq!(project.references.iter().map(|r| (r.kind, r.name.as_str())).collect::<Vec<_>>(), [(ReferenceKind::Look, "The Orchestrator"), (ReferenceKind::Location, "Rail Vault"), (ReferenceKind::Prop, "Lanyard")]);
    for (i, reference) in project.references.iter_mut().enumerate() {
      reference.image = Some(PathBuf::from(format!("{i}.png")));
      reference.version = 2;
    }
    // Props first on disk order doesn't matter: the performer leads.
    project.references.reverse();
    assert_eq!(project.reference_images(), [(ReferenceKind::Look, PathBuf::from("0.png")), (ReferenceKind::Location, PathBuf::from("1.png")), (ReferenceKind::Prop, PathBuf::from("2.png"))]);

    let mut revised = brief();
    revised.venue.description = "A flooded car park.".to_owned();
    project.set_brief(revised);
    assert_eq!(project.references[0].image, Some(PathBuf::from("0.png")), "the performer is unchanged");
    assert_eq!((project.references[1].image.clone(), project.references[1].version), (None, 2), "the venue changed, so its picture goes");
    assert_eq!(project.design(), Design { look: "grainy".into(), character: "Wiry, cable hair.".into(), venue: "A flooded car park.".into() });
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
    assert_eq!(summary, [(ReferenceKind::Look, "The Captain", Some(PathBuf::from("0.png")), 2), (ReferenceKind::Location, "Rail Vault", Some(PathBuf::from("1.png")), 2), (ReferenceKind::Prop, "Lanyard", Some(PathBuf::from("2.png")), 2), (ReferenceKind::Prop, "Ship's wheel", None, 0)], "a prop with nothing written isn't a reference");
    assert_eq!(project.references[0].description, "Wiry, cable hair, a torn greatcoat.");
    assert_eq!(project.design().character, "Wiry, cable hair, a torn greatcoat.");
  }

  #[test]
  fn an_edited_reference_rewords_the_brief() {
    let mut project = SongProject::new();
    project.set_brief(brief());
    project.references[0].description = "Wiry, in a greatcoat.".to_owned();
    project.references[2].description = "A brass compass.".to_owned();
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

  #[test]
  fn a_new_plan_keeps_version_counters_and_drops_old_media() {
    let mut project = SongProject::new();
    project.shots = vec![Shot { draft: Take { still: Some(PathBuf::from("old.png")), clip: Some(PathBuf::from("old.mp4")), synced: None }, still_version: 3, clip_version: 2, ..Default::default() }];
    project.cut = Some(PathBuf::from("cut.mp4"));
    let slots = [slot(0, 0.0, 2.0), slot(1, 2.0, 4.0)];
    let scene = PlannedShot { index: 1, image: "B".into(), motion: "m".into(), beat: "manic".into(), sing: true };
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
    let take = |still: &str, clip: Option<&str>| Take { still: Some(still.into()), clip: clip.map(Into::into), synced: None };
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
      let draft = Take { still: Some("d.png".into()), clip: Some("d.mp4".into()), synced: Some("d_sync.mp4".into()) };
      project.shots = vec![Shot { slot: slot(0, 0.0, 2.0), draft: draft.clone(), finished: Take { still: Some("f.png".into()), clip: Some("f.mp4".into()), synced: None }, ..Default::default() }, Shot { slot: slot(1, 2.0, 4.0), draft, finished: Take { still: Some("f2.png".into()), ..Default::default() }, ..Default::default() }];
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
      let clip = |synced: bool| Take { still: Some("s.png".into()), clip: Some("c.mp4".into()), synced: synced.then(|| "x.mp4".into()) };
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
      assert_eq!(project.shots[0].draft, Take { still: Some("/p/a.png".into()), clip: Some("/p/a.mp4".into()), synced: None });
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
