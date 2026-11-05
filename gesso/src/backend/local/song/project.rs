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

/// One scene of the video: a still, then the clip made from it.
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
  pub still: Option<PathBuf>,
  pub still_version: u32,
  pub clip: Option<PathBuf>,
  pub clip_version: u32,
  /// Why the last still or clip failed.
  pub error: Option<String>,
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
  pub aspect: Aspect,
  pub grade: Grade,
  pub pace: Pace,
  pub shots: Vec<Shot>,
  pub cut: Option<PathBuf>,
  pub cut_version: u32,
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
    Self { id: format!("{ID_PREFIX}{}", uuid::Uuid::new_v4().simple()), created_at: now.clone(), updated_at: now, ..Default::default() }
  }

  /// A saved project by id.
  pub fn load(library_dir: &Path, id: &str) -> Option<Self> {
    let path = library_dir.join(DIRECTORY).join(id).join(FILENAME);
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).map_err(|err| warn!("Ignoring unreadable {}: {err}", path.display())).ok()
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
    } else if self.shots.is_empty() || self.shots.iter().all(|s| s.still.is_none()) {
      // Reference images help but aren't required: scenes can be drawn from words alone.
      Stage::Scenes
    } else if self.shots.iter().all(|s| s.clip.is_none()) {
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

  /// The reference pictures that exist, the performer first: what every still is drawn from.
  pub fn reference_images(&self) -> Vec<PathBuf> {
    let mut made: Vec<&Reference> = self.references.iter().filter(|r| r.image.is_some()).collect();
    made.sort_by_key(|r| match r.kind {
      ReferenceKind::Look => 0,
      ReferenceKind::Location => 1,
      ReferenceKind::Prop => 2,
    });
    made.into_iter().filter_map(|r| r.image.clone()).collect()
  }

  /// Replaces the storyboard with the writer's scenes for `slots`. Stills and clips made for the
  /// old storyboard no longer match, so they're dropped (their files stay on disk).
  pub fn set_plan(&mut self, slots: &[Slot], planned: &[PlannedShot]) {
    self.shots = slots
      .iter()
      .map(|slot| {
        let scene = planned.iter().find(|p| p.index == slot.index);
        let (still_version, clip_version) = self.shots.get(slot.index).map_or((0, 0), |old| (old.still_version, old.clip_version));
        Shot { slot: slot.clone(), image: scene.map(|p| p.image.clone()).unwrap_or_default(), motion: scene.map(|p| p.motion.clone()).unwrap_or_default(), beat: scene.map(|p| p.beat.clone()).unwrap_or_default(), still: None, still_version, clip: None, clip_version, error: None }
      })
      .collect();
    self.cut = None;
  }

  /// The cut's shots: each slot's clip, or its still held when the clip is missing. `None` while
  /// any slot has neither.
  pub fn cut_shots(&self) -> Option<Vec<CutShot>> {
    if self.shots.is_empty() {
      return None;
    }
    self.shots.iter().map(|shot| shot.clip.clone().map(ShotMedia::Clip).or_else(|| shot.still.clone().map(ShotMedia::Still)).map(|media| CutShot { start: shot.slot.start, end: shot.slot.end, media })).collect()
  }

  /// Seconds of video the clips would take to generate, at the clip lengths Veo makes (or
  /// `whole_clips_of` seconds each, for resolutions that only come in one length).
  pub fn seconds_to_generate(&self, only_missing: bool, whole_clips_of: Option<u16>) -> u32 {
    self.shots.iter().filter(|s| s.still.is_some() && (!only_missing || s.clip.is_none())).map(|s| u32::from(whole_clips_of.unwrap_or_else(|| super::plan::clip_seconds(s.slot.seconds())))).sum()
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
    project.shots = vec![Shot { still: Some(PathBuf::from("a.png")), ..Default::default() }];
    assert_eq!(project.furthest(), Stage::Shots);
    project.shots[0].clip = Some(PathBuf::from("a.mp4"));
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
    assert_eq!(project.reference_images(), [PathBuf::from("0.png"), PathBuf::from("1.png"), PathBuf::from("2.png")]);

    let mut revised = brief();
    revised.venue.description = "A flooded car park.".to_owned();
    project.set_brief(revised);
    assert_eq!(project.references[0].image, Some(PathBuf::from("0.png")), "the performer is unchanged");
    assert_eq!((project.references[1].image.clone(), project.references[1].version), (None, 2), "the venue changed, so its picture goes");
    assert_eq!(project.design(), Design { look: "grainy".into(), character: "Wiry, cable hair.".into(), venue: "A flooded car park.".into() });
  }

  #[test]
  fn a_new_plan_keeps_version_counters_and_drops_old_media() {
    let mut project = SongProject::new();
    project.shots = vec![Shot { still: Some(PathBuf::from("old.png")), still_version: 3, clip: Some(PathBuf::from("old.mp4")), clip_version: 2, ..Default::default() }];
    project.cut = Some(PathBuf::from("cut.mp4"));
    let slots = [slot(0, 0.0, 2.0), slot(1, 2.0, 4.0)];
    project.set_plan(&slots, &[PlannedShot { index: 1, image: "B".into(), motion: "m".into(), beat: "manic".into() }]);
    assert_eq!(project.shots.len(), 2);
    assert_eq!((project.shots[0].still.clone(), project.shots[0].still_version, project.shots[0].clip_version), (None, 3, 2));
    assert_eq!((project.shots[1].image.as_str(), project.shots[1].motion.as_str(), project.shots[1].beat.as_str()), ("B", "m", "manic"));
    assert_eq!(project.cut, None);
  }

  #[test]
  fn the_cut_uses_stills_where_clips_are_missing() {
    let mut project = SongProject::new();
    assert_eq!(project.cut_shots(), None);
    project.shots = vec![Shot { slot: slot(0, 0.0, 2.0), clip: Some("a.mp4".into()), still: Some("a.png".into()), ..Default::default() }, Shot { slot: slot(1, 2.0, 7.0), still: Some("b.png".into()), ..Default::default() }];
    let cut = project.cut_shots().unwrap();
    assert_eq!(cut[0].media, ShotMedia::Clip("a.mp4".into()));
    assert_eq!(cut[1], CutShot { start: 2.0, end: 7.0, media: ShotMedia::Still("b.png".into()) });
    // 2 s needs a 4-second clip, 5 s a 6-second one; only the second is missing.
    assert_eq!((project.seconds_to_generate(false, None), project.seconds_to_generate(true, None), project.seconds_to_generate(true, Some(8))), (10, 6, 8));
    project.shots.push(Shot::default());
    assert_eq!(project.cut_shots(), None);
  }

  fn brief() -> Brief {
    let subject = |name: &str, description: &str| Subject { name: name.to_owned(), role: String::new(), description: description.to_owned() };
    Brief { title: "Runtime".into(), character: subject("The Orchestrator", "Wiry, cable hair."), venue: subject("Rail Vault", "A brick tunnel."), props: vec![subject("Lanyard", "A cracked badge."), subject("Empty", " ")], ..Default::default() }
  }

  fn slot(index: usize, start: f64, end: f64) -> Slot {
    Slot { index, start, end, section: "Verse".to_owned(), energy: Energy::Medium, lyric: String::new() }
  }
}
