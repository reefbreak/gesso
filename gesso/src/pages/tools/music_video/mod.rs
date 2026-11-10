//! Music Video: one staged workflow from an idea to a finished film. The stages run down the left
//! rail (Idea, Lyrics, Song, Music Analysis, Visual Style, Creative Brief, Reference Images,
//! Scenes, Shots, Final Video); each opens once the one before it has produced what it needs, and
//! everything a stage makes can be edited before the next one uses it.
//!
//! The project is saved after every change (`~/Artcraft/local/songs/<id>/project.json`), so a
//! video can be picked up later from the list on the first stage.

mod stages;
mod workshop;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use egui::{Align2, FontId, Rect, Sense, Ui, pos2, vec2};

use crate::backend::Backend;
use crate::backend::local::song::plan::{self, Slot};
use crate::backend::local::song::project::{CutOf, Made, Quality, Share, Shot, flows_from, ReferenceKind, SongProject, Stage, Summary, Track};
use crate::backend::local::song::{COST_PARTS, ClipJob, ReferenceJob, SongEstimate, SongRequest, SongUpdate, StillJob, SyncJob, Writer};
use crate::backend::local::song::writer::Turn;
use crate::backend::local::song::cut::ShotMedia;
use crate::backend::local::tools::{Film, FilmScene, VeoTier};
use crate::backend::video::VideoPlayer;
use crate::pages::common::{AppRequest, Env};
use crate::shell::usd;
use crate::theme;
use crate::ui::icons::{self, Icon};
use crate::ui::widgets;
use workshop::{Target, Workshop};

const RAIL_WIDTH: f32 = 212.0;
const GAP: f32 = 16.0;
const ROW_HEIGHT: f32 = 34.0;
/// Edits are written to disk this long after the last keystroke.
const SAVE_AFTER_SECS: f64 = 1.0;
/// Cheapest first: a whole song is dozens of clips.
pub(super) const TIERS: [VeoTier; 3] = [VeoTier::Lite, VeoTier::Fast, VeoTier::Standard];
pub(super) const RESOLUTIONS: [&str; 2] = ["720p", "1080p"];
/// Veo makes 1080p only as 8-second clips.
const HD_CLIP_SECONDS: u16 = 8;
/// Each pass's Veo tier to start with: the cheapest for the draft, the best for the final.
const DEFAULT_TIERS: [usize; 2] = [0, 2];
/// Both passes start at 720p: 1080p costs about twice as much, because every clip is 8 seconds.
const DEFAULT_RESOLUTIONS: [usize; 2] = [0, 0];
/// How often the cost figures are read from the ledger again.
const COSTS_EVERY_SECS: f64 = 2.0;

/// Names a saved project to open at startup.
const OPEN_ENV: &str = "ARTCRAFT_MUSIC_VIDEO";

static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);

/// A stage's run in flight.
struct Run {
  id: u64,
  stage: Stage,
  note: String,
  /// What the run said it was doing when it began; a passing note (a wait for quota) gives way to
  /// it when the next picture or clip arrives.
  started_as: String,
  /// Pictures or clips finished, of how many (zero for single-result stages).
  done: usize,
  total: usize,
  /// Analyse the track as soon as this run delivers one.
  then_analyse: bool,
  /// The pass its stills, clips and syncs belong to.
  quality: Quality,
  /// The scenes whose clip it is making or lip-syncing: the other of the two leaves them alone.
  scenes: BTreeSet<usize>,
  /// Whether it is making clips, the one job lip sync can run beside.
  clips: bool,
}

/// What a stage asked for; applied after the frame is drawn.
pub(super) enum Action {
  Go(Stage),
  New,
  Open(String),
  /// Write the song from the idea, or rewrite the draft following the notes.
  Write {
    rewrite: bool,
  },
  OwnLyrics,
  /// Draw the performer's sheet (the reference at this place) from their head shot.
  SheetFromFace(usize),
  /// Have the writer describe one scene again, keeping the rest.
  PlanScene(usize),
  /// Back to the list of videos.
  Home,
  /// Keep a scene as it stands among its earlier versions.
  KeepScene(usize),
  /// Go back to an earlier version of a scene (the scene, then the version's place in its list).
  RestoreScene(usize, usize),
  /// Go back to an earlier plan of the scenes, by its place in the project's list.
  UsePlan(usize),
  /// Give scenes the pictures still in the folder from an earlier plan.
  Reattach,
  /// Go back to an earlier take of the song, by its place in the project's list.
  UseTake(usize),
  /// Export the cut film's copy for Instagram.
  Share,
  /// Have the writer describe the ending frame of every scene that has none.
  Endings,
  /// Keep the lyrics as they stand among the earlier versions.
  KeepLyrics,
  /// Go back to an earlier version of the lyrics, by its place in the project's list.
  RestoreLyrics(usize),
  Compose,
  Import(PathBuf),
  Analyse,
  Style,
  Brief,
  /// Reference images to make, by index.
  References(Vec<usize>),
  /// The user's own picture for a reference.
  UploadReference(usize, PathBuf),
  /// A written change to a reference's picture.
  ChangeReference(usize, String),
  /// Saves a copy of a reference's picture where the user chose, to edit in another app.
  DownloadReference(usize, PathBuf),
  /// Opens a conversation with the writer about one section.
  Workshop(Target),
  /// Sends what was typed in that conversation.
  WorkshopSend,
  /// Puts the conversation's version of the section back into its box.
  WorkshopApply,
  WorkshopClose,
  /// Stops the lip sync, leaving anything else running.
  StopSync,
  /// Opens the film in Edit Video as its scenes and soundtrack.
  EditFilm,
  Plan,
  /// Scenes whose stills to draw, by index; `true` to use the better model in the draft pass;
  /// and which of each scene's frames.
  Stills(Vec<usize>, bool, Frames),
  /// Scenes whose clips to make, by index.
  Clips(Vec<usize>),
  /// Scenes whose clips to lip-sync, by index.
  LipSync(Vec<usize>),
  /// A vocal stem the user already has.
  ImportVocals(PathBuf),
  /// Switch between the draft and final passes.
  Pass(Quality),
  Cut,
  Stop,
  Reveal(PathBuf),
}

/// What every saved music video has cost, all time, by title: its counted calls plus the
/// estimate for work done before they were counted. For the System Health page.
pub fn video_costs(backend: &Backend) -> Vec<(String, f64)> {
  let Some(library_dir) = backend.library_dir() else {
    return Vec::new();
  };
  let counted = backend.song_costs();
  SongProject::list(&library_dir)
    .into_iter()
    .map(|summary| {
      let earlier = summary.earlier_cost.unwrap_or_else(|| price_made(backend, &summary.made));
      let cost: f64 = counted.get(&summary.id).into_iter().flatten().chain(&earlier).map(|(_, usd)| usd).sum();
      (summary.title, cost)
    })
    .collect()
}

/// A picture a still run is to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PlannedFrame {
  pub index: usize,
  pub ending: bool,
  /// The frame of the same run it is matched to, and so drawn after.
  pub after: Option<(usize, bool)>,
}

/// The pictures a run asked for `frames` of these scenes draws in a pass, in scene order.
///
/// A scene is an opening still and an ending frame. The final pass redraws drafts, so each needs
/// its draft. An ending is drawn to match its opening, so it needs one (on file, or drawn first
/// in the same run) and is drawn again whenever its opening is. A flowing scene has no opening of
/// its own: it opens on the ending of the scene before, so its ending follows that one.
pub(super) fn plan_frames(shots: &[Shot], indices: &[usize], quality: Quality, frames: Frames) -> Vec<PlannedFrame> {
  let mut indices = indices.to_vec();
  indices.sort_unstable();
  indices.dedup();
  let mut plan: Vec<PlannedFrame> = Vec::new();
  for index in indices {
    let Some(shot) = shots.get(index).filter(|shot| !shot.image.trim().is_empty()) else {
      continue;
    };
    // Where its opening comes from: a frame of this run, or one on file.
    let (matched, on_file) = if flows_from(shots, index) {
      let before = (index - 1, true);
      (plan.iter().any(|frame| (frame.index, frame.ending) == before).then_some(before), shots[index - 1].take(quality).end_still.is_some())
    } else {
      let opening = match frames {
        Frames::Due => shot.opening_due(quality),
        Frames::Both | Frames::Opening => true,
        Frames::Ending => false,
      } && (quality == Quality::Draft || shot.draft.still.is_some());
      if opening {
        plan.push(PlannedFrame { index, ending: false, after: None });
      }
      (opening.then_some((index, false)), shot.take(quality).still.is_some())
    };
    let asked = match frames {
      Frames::Due => matched.is_some() || shot.end_due(quality),
      Frames::Both | Frames::Ending => true,
      Frames::Opening => false,
    };
    let drawable = (matched.is_some() || on_file) && (quality == Quality::Draft || shot.draft.end_still.is_some());
    if asked && shot.wants_ending() && drawable {
      plan.push(PlannedFrame { index, ending: true, after: matched });
    }
  }
  plan
}

/// How many pictures a run asked for `frames` of these scenes draws: openings, then endings.
pub(super) fn frame_count(shots: &[Shot], indices: &[usize], quality: Quality, frames: Frames) -> (usize, usize) {
  let plan = plan_frames(shots, indices, quality, frames);
  let endings = plan.iter().filter(|frame| frame.ending).count();
  (plan.len() - endings, endings)
}

/// The price, part by part, of what a video's folder holds: the estimate for work done before
/// each video's calls were counted. Clips are priced at the page's default model for their pass.
fn price_made(backend: &Backend, made: &Made) -> Vec<(String, f64)> {
  let price = |estimate: SongEstimate| backend.estimate_song(&estimate).unwrap_or(0.0);
  let mut parts = vec![(COST_PARTS[1], made.songs as f64 * price(SongEstimate::Song { clip: false })), (COST_PARTS[2], price(SongEstimate::Stills { count: made.references, pro: false, large: false })), (COST_PARTS[3], price(SongEstimate::Stills { count: made.stills[0], pro: false, large: false }) + price(SongEstimate::Stills { count: made.stills[1], pro: true, large: true }))];
  let mut clips = 0.0;
  let mut sync = 0.0;
  for pass in 0..2 {
    let resolution = RESOLUTIONS[DEFAULT_RESOLUTIONS[pass]];
    // 1080p clips are always eight seconds long.
    let seconds = if resolution == "1080p" { made.clips[pass] as u32 * 8 } else { made.clip_seconds[pass] };
    clips += price(SongEstimate::Clips { seconds, tier: TIERS[DEFAULT_TIERS[pass]], resolution: resolution.to_owned() });
    sync += price(SongEstimate::LipSync { seconds: made.synced_seconds[pass], pro: pass == 1, separate_seconds: if pass == 0 { made.separated_seconds } else { 0.0 } });
  }
  parts.push((COST_PARTS[4], clips));
  parts.push((COST_PARTS[5], sync));
  parts.into_iter().filter(|(_, usd)| *usd > 0.0).map(|(part, usd)| (part.to_owned(), usd)).collect()
}

/// Something for the app to say after an update.
pub struct Notice {
  pub error: bool,
  pub message: String,
  /// The library gained a file (the song, a reference image or the finished film).
  pub library_changed: bool,
}

/// Which of a scene's frames a run draws: a scene has an opening frame and, when it asks for
/// one, an ending frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Frames {
  /// Whatever is missing or out of date.
  Due,
  /// Both, again.
  Both,
  Opening,
  Ending,
}

pub struct MusicVideoPage {
  project: SongProject,
  /// The stage on screen.
  stage: Stage,
  run: Option<Run>,
  /// A lip sync, which runs on fal and so can go on while Veo makes clips.
  sync: Option<Run>,
  error: Option<String>,
  /// The last analysis's caveats.
  analysis_note: Option<String>,
  /// Saved projects, listed on the first stage (`None` until read).
  recent: Option<Vec<Summary>>,
  writer: Writer,
  /// What to change in a rewrite.
  notes: String,
  instrumental: bool,
  /// The Veo tier and resolution picked for each pass (draft, then final), as indices into
  /// [`TIERS`] and [`RESOLUTIONS`].
  tier: [usize; 2],
  resolution: [usize; 2],
  /// The scene picked in the Scenes and Shots grids.
  selected: Option<usize>,
  /// Redraw picked draft stills with Nano Banana Pro instead of the draft's faster model.
  better_redraw: bool,
  /// The scenes selected together on the storyboard, to redraw in one go.
  picked: BTreeSet<usize>,
  player: Option<VideoPlayer>,
  /// The section being worked on with the writer, if any.
  workshop: Option<Workshop>,
  /// What to change in each reference's picture, as typed under it.
  reference_changes: Vec<String>,
  /// The slots a plan in flight was asked for.
  pending_slots: Vec<Slot>,
  /// What a cut in flight is being made from.
  pending_cut: Option<CutOf>,
  /// When the project last changed without being saved.
  dirty_since: Option<f64>,
  /// A project to open on the first frame (`ARTCRAFT_MUSIC_VIDEO`, for screenshots).
  open_on_start: Option<String>,
  /// Whether the writer has been asked for the ending frames this video's scenes lack, so a
  /// failed attempt isn't repeated on every frame.
  endings_asked: bool,
  /// The page's front: the list of videos, shown until one is opened or started.
  pub(super) home: bool,
  /// What the director asks of the scene being written again.
  pub(super) scene_note: String,
  /// The earlier version of the selected scene that is open, by its place in the scene's list.
  pub(super) viewing_scene_version: Option<usize>,
  /// The earlier plan of the scenes that is open, by its place in the project's list.
  pub(super) viewing_plan: Option<usize>,
  /// The earlier take of the song that is open to listen to, by its place in the project's list.
  pub(super) viewing_take: Option<usize>,
  /// The earlier version of the lyrics that is open to read, by its place in the project's list.
  pub(super) viewing_lyrics: Option<usize>,
  /// What each video's calls have cost, part by part, and when that was last read.
  costs: Option<(f64, BTreeMap<String, Vec<(String, f64)>>)>,
}

impl Default for MusicVideoPage {
  fn default() -> Self {
    Self { project: SongProject::new(), stage: Stage::Idea, run: None, sync: None, error: None, analysis_note: None, recent: None, writer: Writer::default(), notes: String::new(), instrumental: false, tier: DEFAULT_TIERS, resolution: DEFAULT_RESOLUTIONS, selected: None, better_redraw: false, picked: BTreeSet::new(), player: None, workshop: None, reference_changes: Vec::new(), pending_slots: Vec::new(), pending_cut: None, dirty_since: None, open_on_start: None, home: true, scene_note: String::new(), viewing_scene_version: None, viewing_plan: None, viewing_take: None, viewing_lyrics: None, endings_asked: false, costs: None }
  }
}

impl MusicVideoPage {
  /// The page as the app starts: `ARTCRAFT_MUSIC_VIDEO=<project id>` opens that saved project
  /// (with `ARTCRAFT_PAGE=music-video`, for screenshots).
  pub fn from_env() -> Self {
    let open_on_start = std::env::var(OPEN_ENV).ok().filter(|id| !id.is_empty());
    Self { home: open_on_start.is_none(), open_on_start, ..Self::default() }
  }

  pub fn ui(&mut self, ui: &mut Ui, env: &mut Env<'_>) {
    let area = ui.max_rect().shrink(GAP);
    let rail = Rect::from_min_max(area.min, pos2(area.left() + RAIL_WIDTH, area.bottom()));
    let panel = Rect::from_min_max(pos2(area.right() - super::PANEL_WIDTH, area.top()), area.max);
    let stage = Rect::from_min_max(pos2(rail.right() + GAP, area.top()), pos2(panel.left() - GAP, area.bottom()));
    if self.recent.is_none() {
      self.recent = Some(env.backend.library_dir().map(|dir| SongProject::list(&dir)).unwrap_or_default());
    }
    if let Some(id) = std::mem::take(&mut self.open_on_start) {
      self.apply(env, Action::Open(id));
      // For screenshots: a scene to open in the editor.
      self.selected = std::env::var("ARTCRAFT_MUSIC_VIDEO_SCENE").ok().and_then(|n| n.parse::<usize>().ok()).and_then(|n| n.checked_sub(1)).filter(|n| *n < self.project.shots.len());
    }
    self.count_costs(ui, env.backend);
    // A flowing scene opens on the ending frame before it, whenever that frame is new.
    if self.project.follow_flow() {
      self.dirty_since.get_or_insert(ui.input(|i| i.time));
    }
    let mut actions = Vec::new();
    // Every scene has an ending frame: ones planned before that was so are given theirs when
    // the storyboard is next opened.
    if self.stage == Stage::Scenes && !self.endings_asked && !self.busy() && self.project.brief.is_some() && !self.project.scenes_without_ending().is_empty() {
      self.endings_asked = true;
      actions.push(Action::Endings);
    }
    if self.home {
      // The front of the page: every video, to open, and a new one to start.
      let whole = Rect::from_min_max(pos2(rail.right() + GAP, area.top()), area.max);
      stages::home(self, ui, env, whole, &mut actions);
      self.rail(ui, rail, &mut actions);
      for action in actions {
        self.apply(env, action);
      }
      return;
    }
    self.rail(ui, rail, &mut actions);
    stages::show(self, ui, env, stage, panel, &mut actions);
    workshop::show(self, ui, &mut actions);
    for action in actions {
      self.apply(env, action);
    }
    self.autosave(ui, env.backend);
  }

  /// Keeps the cost figures fresh: the ledger is read again every couple of seconds, and a video
  /// made before its calls were counted is priced once from the files in its folder.
  fn count_costs(&mut self, ui: &Ui, backend: &Backend) {
    let now = ui.input(|i| i.time);
    if self.costs.as_ref().is_none_or(|(read, _)| now - read >= COSTS_EVERY_SECS) {
      self.costs = Some((now, backend.song_costs()));
    }
    if self.busy() {
      ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(COSTS_EVERY_SECS));
    }
    if self.project.earlier_cost.is_none() {
      if let Some(library_dir) = backend.library_dir() {
        let made = self.project.made(&self.project.dir(&library_dir));
        self.project.earlier_cost = Some(price_made(backend, &made));
        if !made.is_empty() {
          self.dirty_since.get_or_insert(now);
        }
      }
    }
  }

  /// What a video has cost so far, part by part in [`COST_PARTS`]' order: its counted calls, plus
  /// `earlier`, the estimate for what was made before they were counted.
  pub(super) fn cost_of(&self, id: &str, earlier: &[(String, f64)]) -> Vec<(&'static str, f64)> {
    let counted = self.costs.as_ref().and_then(|(_, costs)| costs.get(id));
    COST_PARTS.iter().map(|part| (*part, counted.into_iter().flatten().chain(earlier).filter(|(name, _)| name == part).map(|(_, usd)| usd).sum::<f64>())).filter(|(_, usd)| *usd > 0.0).collect()
  }

  /// The rail's cost block: this video's total and what it's made of. Every video together is
  /// on the System Health page.
  fn cost_block(&self, ui: &mut Ui) {
    let earlier = self.project.earlier_cost.clone().unwrap_or_default();
    let parts = self.cost_of(&self.project.id, &earlier);
    let total: f64 = parts.iter().map(|(_, usd)| usd).sum();
    widgets::hud(ui, "Cost so far", theme::FAINT);
    ui.add_space(2.0);
    ui.label(egui::RichText::new(usd(total)).size(20.0).family(theme::mono_bold()).color(theme::INK));
    for (part, cost) in &parts {
      let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::hover());
      ui.painter().text(row.left_center(), Align2::LEFT_CENTER, *part, FontId::new(11.5, theme::medium()), theme::MUTED);
      ui.painter().text(row.right_center(), Align2::RIGHT_CENTER, usd(*cost), FontId::new(11.5, theme::mono()), theme::MUTED);
    }
    let estimated: f64 = earlier.iter().map(|(_, usd)| usd).sum();
    let mut note = "At list prices, counted as each call returns.".to_owned();
    if estimated > 0.0 {
      note = format!("At list prices. {} of it is an estimate from the files made before Gesso counted each video's calls.", usd(estimated));
    }
    ui.add_space(2.0);
    ui.label(egui::RichText::new(note).size(11.0).color(theme::FAINT));
    ui.add_space(12.0);
  }

  /// What a saved video has cost so far.
  pub(super) fn cost_of_saved(&self, backend: &Backend, summary: &Summary) -> f64 {
    let earlier = summary.earlier_cost.clone().unwrap_or_else(|| price_made(backend, &summary.made));
    self.cost_of(&summary.id, &earlier).iter().map(|(_, usd)| usd).sum()
  }

  /// Takes news from a run; `None` when it isn't this page's or needs no toast.
  pub fn on_update(&mut self, backend: &Backend, run_id: u64, update: SongUpdate) -> Option<Notice> {
    let sync = self.sync.as_ref().map(|r| r.id) == Some(run_id);
    if !sync && self.run.as_ref().map(|r| r.id) != Some(run_id) {
      return None;
    }
    let mut notice = None;
    let current = self.project.quality;
    let quality = self.slot(sync).as_ref().map_or(current, |run| run.quality);
    let library = |message: &str| Some(Notice { error: false, message: message.to_owned(), library_changed: true });
    match update {
      SongUpdate::Written(draft) => {
        self.project.keep_lyrics("Before a rewrite");
        self.project.title = draft.title;
        self.project.style = draft.style;
        self.project.lyrics = draft.lyrics;
        self.notes.clear();
        self.go(Stage::Lyrics);
      },
      SongUpdate::Track { path, model, sung } => {
        // A new track means new beats, so everything timed to the old one goes.
        let generated = !model.is_empty();
        self.project.in_library.insert(path.clone());
        // The take it replaces is set aside with its scenes, and can be gone back to.
        self.project.set_track(Track { path, model, sung });
        self.selected = None;
        self.player = None;
        if generated {
          notice = library("The song is in your library");
        }
      },
      SongUpdate::Analysed { grid, sections, read, lines, note } => {
        self.project.grid = Some(grid);
        self.project.sections = sections;
        self.project.read = read;
        self.project.lines = lines;
        self.analysis_note = note;
      },
      SongUpdate::Styled(style) => self.project.visual = Some(style),
      SongUpdate::Briefed(brief) => self.project.set_brief(brief),
      SongUpdate::Workshopped(revision) => {
        if let Some(workshop) = &mut self.workshop {
          workshop.fields = revision.fields;
          workshop.turns.push(Turn { from_user: false, text: revision.reply });
        }
      },
      SongUpdate::Reference { index, path } => {
        self.project.in_library.insert(path.clone());
        if let Some(reference) = self.project.references.get_mut(index) {
          reference.image = Some(path);
          reference.error = None;
        }
        self.progress(sync);
        notice = Some(Notice { error: false, message: String::new(), library_changed: true });
      },
      SongUpdate::ReferenceFailed { index, message } => {
        if let Some(reference) = self.project.references.get_mut(index) {
          reference.error = Some(message);
        }
        self.progress(sync);
      },
      SongUpdate::Endings(endings) => self.project.set_endings(&endings),
      SongUpdate::Replanned(index, scene) => {
        self.project.set_scene(index, &scene);
        notice = Some(Notice { error: false, message: format!("Scene {} is rewritten. Its pictures were drawn to the old words: redraw them when you're happy with it.", index + 1), library_changed: false });
      },
      SongUpdate::Shared { path, of, kbps, bytes } => {
        self.project.in_library.insert(path.clone());
        self.project.share = Some(Share { path, of, kbps, bytes });
        notice = library("The Instagram copy is in your library");
      },
      SongUpdate::Planned(scenes) => {
        let slots = std::mem::take(&mut self.pending_slots);
        self.project.set_plan(&slots, &scenes);
        self.selected = None;
        self.picked.clear();
      },
      SongUpdate::Still { index, path, ending } => {
        if let Some(shot) = self.project.shots.get_mut(index) {
          // The clip and lip sync made from the old still stay, marked out of date, until they
          // are made again.
          self.project.in_library.insert(path.clone());
          shot.keep(if ending { "Before a new ending frame" } else { "Before a new opening frame" });
          if ending {
            shot.set_end_still(quality, path);
          } else {
            shot.set_still(quality, path);
          }
          shot.error = None;
          shot.clip_error = None;
        }
        self.progress(sync);
      },
      SongUpdate::Clip { index, path } => {
        if let Some(shot) = self.project.shots.get_mut(index) {
          // A lip sync of the old clip stays, marked out of date.
          self.project.in_library.insert(path.clone());
          shot.keep("Before a new clip");
          shot.set_clip(quality, path);
          shot.clip_error = None;
        }
        if self.selected == Some(index) {
          self.player = None;
        }
        // Its clip is in, so the scene is free to be lip-synced while the run goes on.
        if let Some(run) = &mut self.run {
          run.scenes.remove(&index);
        }
        self.progress(sync);
      },
      SongUpdate::Synced { index, path } => {
        if let Some(shot) = self.project.shots.get_mut(index) {
          self.project.in_library.insert(path.clone());
          shot.keep("Before a new lip sync");
          shot.set_synced(quality, path);
          shot.clip_error = None;
        }
        if self.selected == Some(index) {
          self.player = None;
        }
        self.progress(sync);
      },
      SongUpdate::Vocals(path) => {
        self.project.in_library.insert(path.clone());
        self.project.vocals = Some(path);
      },
      SongUpdate::ShotFailed { index, message } => {
        // A failed clip or lip sync belongs to the Shots stage; the scene's still is fine.
        let on_clip = self.slot(sync).as_ref().is_some_and(|run| run.stage == Stage::Shots);
        if let Some(shot) = self.project.shots.get_mut(index) {
          if on_clip {
            shot.clip_error = Some(message);
          } else {
            shot.error = Some(message);
          }
        }
        // A clip that failed isn't being made any more.
        if let Some(run) = self.slot(sync).as_mut().filter(|run| run.clips) {
          run.scenes.remove(&index);
        }
        self.progress(sync);
      },
      SongUpdate::Catalogued(paths) => {
        if !paths.is_empty() {
          notice = library(&format!("{} file{} from this video added to your library", paths.len(), if paths.len() == 1 { "" } else { "s" }));
        }
        self.project.in_library.extend(paths);
      },
      SongUpdate::Cut(path) => {
        self.project.in_library.insert(path.clone());
        self.project.cut_of = self.pending_cut.take();
        self.project.cut = Some(path);
        self.player = None;
        notice = library("The film is in your library");
      },
      SongUpdate::Note(note) => {
        if let Some(run) = self.slot(sync) {
          run.note = note;
        }
      },
      SongUpdate::Done(result) => {
        let finished = self.slot(sync).take();
        if let Some(workshop) = self.workshop.as_mut().filter(|w| w.waiting) {
          workshop.waiting = false;
          workshop.error = result.as_ref().err().cloned();
        }
        match result {
          Ok(()) => {
            if finished.is_some_and(|run| run.then_analyse) && self.project.track.is_some() {
              self.go(Stage::Analysis);
              self.analyse(backend);
            }
          },
          Err(message) => {
            self.error = Some(message.clone());
            notice = Some(Notice { error: true, message, library_changed: false });
          },
        }
      },
    }
    self.save(backend);
    notice.filter(|n| !n.message.is_empty() || n.library_changed)
  }

  /// Whether anything is running, a lip sync included: what locks the rest of the page.
  pub(super) fn busy(&self) -> bool {
    self.run.is_some() || self.sync.is_some()
  }

  /// Whether clips can be started: nothing but a lip sync is running.
  pub(super) fn can_make_clips(&self) -> bool {
    self.run.is_none()
  }

  /// Whether a lip sync can be started: none is running, and nothing else is but clips being made.
  pub(super) fn can_sync(&self) -> bool {
    self.sync.is_none() && self.run.as_ref().is_none_or(|run| run.clips)
  }

  /// The sung scenes whose clips the run in hand hasn't delivered yet. Lip sync waits for them.
  pub(super) fn sung_being_filmed(&self) -> usize {
    self.run.as_ref().filter(|run| run.clips).map_or(0, |run| run.scenes.iter().filter(|index| self.project.shots.get(**index).is_some_and(|shot| shot.sing)).count())
  }

  /// The lip sync's progress line, while one runs.
  pub(super) fn sync_line(&self) -> Option<String> {
    self.sync.as_ref().map(|run| if run.total > 0 { format!("{} {} of {}", run.note, run.done, run.total) } else { run.note.clone() })
  }

  /// Whether the stage on screen is the one running.
  pub(super) fn running_here(&self) -> bool {
    self.run.as_ref().is_some_and(|run| run.stage == self.stage)
  }

  /// The main run, or the lip sync beside it.
  fn slot(&mut self, sync: bool) -> &mut Option<Run> {
    if sync { &mut self.sync } else { &mut self.run }
  }

  /// The run's progress line ("Drawing stills… 12 of 64").
  pub(super) fn progress_line(&self) -> Option<String> {
    let run = self.run.as_ref()?;
    Some(if run.total > 0 { format!("{} {} of {}", run.note, run.done, run.total) } else { run.note.clone() })
  }

  /// The length each clip is generated at, when the resolution fixes it.
  pub(super) fn whole_clips_of(&self) -> Option<u16> {
    (self.resolution() == "1080p").then_some(HD_CLIP_SECONDS)
  }

  /// The pass being worked on.
  pub(super) fn quality(&self) -> Quality {
    self.project.quality
  }

  /// The current pass's place in the per-pass settings.
  pub(super) fn pass(&self) -> usize {
    usize::from(self.project.quality == Quality::Final)
  }

  pub(super) fn tier(&self) -> VeoTier {
    TIERS[self.tier[self.pass()]]
  }

  pub(super) fn resolution(&self) -> &'static str {
    RESOLUTIONS[self.resolution[self.pass()]]
  }

  /// Pictures are drawn with Nano Banana Pro in the final pass.
  pub(super) fn pro(&self) -> bool {
    self.project.quality == Quality::Final
  }

  /// The film as its parts, for Edit Video: each scene's clip (or still) at its length in the
  /// cut, in order, with the song, at the size, finish and fades the cut here would use.
  pub(super) fn film(&self) -> Option<Film> {
    let shots = self.project.cut_shots(self.quality())?;
    let scenes = shots
      .into_iter()
      .zip(&self.project.shots)
      .enumerate()
      .map(|(i, (cut, shot))| {
        let (media, still) = match cut.media {
          ShotMedia::Clip(path) => (path, false),
          ShotMedia::Still(path) => (path, true),
        };
        FilmScene { label: format!("{} \u{b7} {}", i + 1, shot.slot.section), media, still, seconds: cut.end - cut.start, lyric: shot.slot.lyric.clone() }
      })
      .collect();
    Some(Film { title: self.project.display_title(), scenes, audio: Some(self.project.track.as_ref()?.path.clone()), size: self.project.format.size(self.project.aspect, self.resolution()), grade: self.project.grade, fades: self.project.fades })
  }

  /// What this pass still has to make or make again, in the order it is made: stills, clips,
  /// lip syncs, the film. Each line goes to the stage that does it. A change to one scene's still
  /// shows here as the clip, the lip sync and the film it has overtaken.
  fn to_update(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
    if self.project.shots.is_empty() {
      return;
    }
    let lineage = self.project.lineage(self.project.quality);
    widgets::hud(ui, "To update", theme::FAINT);
    ui.add_space(4.0);
    if lineage.settled() {
      ui.label(egui::RichText::new("Everything in this pass is made and up to date.").size(11.5).color(theme::FAINT));
      ui.add_space(12.0);
      return;
    }
    let counted = |missing: usize, stale: usize, one: &str, many: &str| {
      let name = |n: usize| if n == 1 { one } else { many };
      match (missing, stale) {
        (0, stale) => format!("{stale} {} out of date", name(stale)),
        (missing, 0) => format!("{missing} {} to make", name(missing)),
        (missing, stale) => format!("{missing} {} to make, {stale} out of date", name(missing)),
      }
    };
    let film = match (self.project.cut.is_some(), lineage.film_current) {
      (true, Some(false)) => Some("The film is out of date".to_owned()),
      (true, None) => Some("The film may be out of date".to_owned()),
      _ => None,
    };
    let lines = [
      (Stage::Scenes, (lineage.stills() > 0).then(|| counted(lineage.stills_missing, lineage.stills_stale, "still", "stills"))),
      (Stage::Shots, (lineage.clips() > 0).then(|| counted(lineage.clips_missing, lineage.clips_stale, "clip", "clips"))),
      (Stage::Shots, (lineage.syncs() > 0).then(|| counted(lineage.syncs_missing, lineage.syncs_stale, "lip sync", "lip syncs"))),
      (Stage::Final, film),
    ];
    for (stage, line) in lines {
      let Some(line) = line else {
        continue;
      };
      let text = egui::RichText::new(format!("{line} \u{2192} {}", stage.label())).size(11.5).color(theme::BADGE_BETA);
      if ui.add(egui::Button::new(text).frame(false).wrap()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(format!("Go to {}", stage.label())).clicked() {
        actions.push(Action::Go(stage));
      }
    }
    ui.add_space(12.0);
  }

  /// The left rail: the project's name and the stages, each with its state.
  fn rail(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
    ui.painter().rect(rect, theme::RADIUS, theme::PANEL, theme::hairline(), egui::StrokeKind::Inside);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(12.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut child;
    // The rail scrolls: with the list of what needs updating it can be taller than the window.
    egui::ScrollArea::vertical().id_salt("mv-rail").auto_shrink([false, false]).show(ui, |ui| {
      widgets::hud(ui, "Music video", theme::FAINT);
      ui.add_space(4.0);
      if self.home {
        ui.label(widgets::heading("Your videos", 18.0));
        ui.add_space(10.0);
        ui.add_enabled_ui(!self.busy(), |ui| {
          if widgets::button(ui, Some(Icon::Plus), "New video", widgets::Kind::Accent, ui.available_width(), 30.0).clicked() {
            actions.push(Action::New);
          }
        });
        return;
      }
      // Back to the list; the video is saved as it stands.
      if widgets::ghost(ui, Icon::ChevronLeft, Some("All videos"), "Back to the list of your videos", false).clicked() {
        actions.push(Action::Home);
      }
      ui.add_space(6.0);
      ui.label(widgets::heading(&self.project.display_title(), 18.0));
      ui.add_space(10.0);
      let furthest = self.project.furthest();
      for stage in Stage::ALL {
        let reachable = self.project.reachable(stage);
        let current = stage == self.stage;
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), if reachable { Sense::click() } else { Sense::hover() });
        let hovered = resp.hovered() && reachable;
        if current || hovered {
          ui.painter().rect_filled(row, theme::RADIUS, if current { theme::WASH_SELECTED } else { theme::WASH });
        }
        let ink = if current {
          theme::INK
        } else if reachable {
          theme::SIDEBAR_INK
        } else {
          theme::FAINT
        };
        let mark = Rect::from_center_size(pos2(row.left() + 16.0, row.center().y), vec2(14.0, 14.0));
        let running = self.run.iter().chain(&self.sync).any(|run| run.stage == stage);
        if running {
          icons::paint_spinner(ui, mark, theme::ACCENT_400);
        } else if stage < furthest || (stage == Stage::Final && self.project.cut.is_some()) {
          icons::paint(ui.painter(), mark, Icon::Check, theme::ACCENT_400);
        } else {
          ui.painter().text(mark.center(), Align2::CENTER_CENTER, stage.number().to_string(), FontId::new(11.0, theme::mono()), ink);
        }
        ui.painter().text(pos2(row.left() + 34.0, row.center().y), Align2::LEFT_CENTER, stage.label(), FontId::new(13.5, if current { theme::semibold() } else { theme::medium() }), ink);
        let resp = if reachable { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp.on_hover_text("Finish the stage before this one first") };
        if resp.clicked() && reachable {
          actions.push(Action::Go(stage));
        }
      }
      ui.add_space(12.0);
      // Draft or final: which models the pictures and clips use, and which set is on screen.
      widgets::hud(ui, "Quality", theme::FAINT);
      ui.add_space(4.0);
      let busy = self.busy();
      ui.add_enabled_ui(!busy, |ui| {
        if let Some(i) = super::segmented(ui, &Quality::ALL.map(Quality::label), self.pass()) {
          actions.push(Action::Pass(Quality::ALL[i]));
        }
      });
      ui.add_space(2.0);
      let hint = match self.project.quality {
        Quality::Draft => "Cheap, fast models, to settle the direction.",
        Quality::Final => "The best models: each still is redrawn from its draft, then filmed again.",
      };
      ui.label(egui::RichText::new(hint).size(11.5).color(theme::FAINT));
      ui.add_space(12.0);
      self.cost_block(ui);
      self.to_update(ui, actions);
      if let Some(line) = self.progress_line() {
        ui.label(egui::RichText::new(line).size(12.0).color(theme::MUTED));
        if let Some(run) = self.run.as_ref().filter(|run| run.total > 0) {
          let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 3.0), Sense::hover());
          widgets::progress_bar(ui.painter(), bar, run.done as f32 / run.total as f32);
        }
        ui.add_space(8.0);
      }
      ui.add_enabled_ui(!self.busy(), |ui| {
        if widgets::button(ui, Some(Icon::Plus), "New video", widgets::Kind::Secondary, ui.available_width(), 30.0).clicked() {
          actions.push(Action::New);
        }
      });
    });
  }

  fn apply(&mut self, env: &mut Env<'_>, action: Action) {
    let backend = env.backend;
    let Some(library_dir) = backend.library_dir() else {
      return;
    };
    let dir = self.project.dir(&library_dir);
    match action {
      Action::Go(stage) => self.go(stage),
      Action::Home => {
        self.save(backend);
        self.home = true;
        self.recent = None;
        self.player = None;
      },
      Action::New => {
        self.save(backend);
        *self = Self { writer: self.writer, tier: self.tier, resolution: self.resolution, home: false, ..Self::default() };
      },
      Action::Open(id) => {
        self.home = false;
        self.save(backend);
        if let Some(project) = SongProject::load(&library_dir, &id) {
          let stage = if project.reachable(project.stage) { project.stage } else { project.furthest() };
          *self = Self { project, stage, writer: self.writer, tier: self.tier, resolution: self.resolution, ..Self::default() };
          // Songs made before earlier takes were kept are still in the folder: list them.
          let dir = self.project.dir(&library_dir);
          self.project.adopt_song_files(&dir);
          // Everything the video has made belongs in the library, whenever it was made.
          let request = SongRequest::Catalogue { dir: self.project.dir(&library_dir), known: self.project.in_library.clone() };
          self.start(backend, 0, "Checking this video's files are in the library\u{2026}", request);
        }
      },
      Action::Write { rewrite } => {
        let current = rewrite.then(|| self.project.draft());
        let note = if rewrite { "Rewriting the song\u{2026}" } else { "Writing the song\u{2026}" };
        self.start(backend, 0, note, SongRequest::Write { idea: self.project.idea.clone(), notes: self.notes.clone(), current, length: self.project.song_length, writer: self.writer });
      },
      Action::OwnLyrics => {
        if self.project.lyrics.trim().is_empty() {
          self.project.lyrics = "[Verse 1]\n".to_owned();
        }
        self.go(Stage::Lyrics);
      },
      Action::KeepLyrics => {
        self.project.keep_lyrics("Kept by hand");
        self.save(backend);
      },
      Action::RestoreLyrics(index) => {
        self.project.restore_lyrics(index);
        self.viewing_lyrics = None;
        self.save(backend);
      },
      Action::Compose => {
        let version = self.next_version();
        self.project.keep_lyrics(&format!("Used for song take {version}"));
        self.start(backend, 0, "Lyria is making the song (a minute or two)\u{2026}", SongRequest::Compose { dir, draft: self.project.draft(), length: self.project.song_length, instrumental: self.instrumental, version });
        self.analyse_after();
      },
      Action::Import(file) => {
        self.start(backend, 0, "Bringing in the track\u{2026}", SongRequest::Import { dir, file });
        self.analyse_after();
      },
      Action::Analyse => self.analyse(backend),
      Action::Style => self.start(backend, 0, "Designing the visual style\u{2026}", SongRequest::Style { idea: self.project.idea.clone(), draft: self.project.draft(), read: self.project.read.clone(), direction: self.project.direction.clone(), writer: self.writer }),
      Action::Brief => {
        if let Some(style) = self.project.visual.clone() {
          self.start(backend, 0, "Writing the creative brief\u{2026}", SongRequest::Brief { idea: self.project.idea.clone(), draft: self.project.draft(), read: self.project.read.clone(), style, writer: self.writer });
        }
      },
      Action::References(indices) => {
        let mut items = Vec::new();
        // The head shot is drawn from the sheet. When the sheet is being drawn in the same go, the
        // head shot waits for it: the button then offers it as the one still missing.
        let sheet = self.project.references.iter().position(|r| r.kind == ReferenceKind::Look);
        let sheet_picture = sheet.and_then(|i| self.project.references[i].image.clone());
        let sheet_in_this_run = sheet.is_some_and(|i| indices.contains(&i));
        // It also works the other way: a head shot that leads (an uploaded one) is the
        // performer, and the sheet is drawn from it. Drawing everything then leaves it alone.
        let leading_face = self.project.leading_face();
        let several = indices.len() > 1;
        for index in indices {
          if let Some(reference) = self.project.references.get_mut(index) {
            if reference.kind == ReferenceKind::Face && (sheet_in_this_run || (several && leading_face.is_some())) {
              continue;
            }
            reference.version += 1;
            reference.error = None;
            // A head shot drawn again from the sheet follows the sheet from then on.
            reference.leads = false;
            let sheet = match reference.kind {
              ReferenceKind::Face => sheet_picture.clone(),
              ReferenceKind::Look => leading_face.clone(),
              _ => None,
            };
            items.push(ReferenceJob { index, version: reference.version, kind: reference.kind, name: reference.name.clone(), description: reference.description.clone(), change: None, sheet });
          }
        }
        if !items.is_empty() {
          let look = self.project.design().look;
          self.start(backend, items.len(), "Drawing reference images\u{2026}", SongRequest::References { dir, look, aspect: self.project.aspect, pro: self.pro(), items });
        }
      },
      Action::Workshop(target) => self.workshop = Workshop::open(&self.project, target),
      Action::WorkshopSend => {
        let busy = self.busy();
        let request = self.workshop.as_mut().filter(|w| !busy && !w.input.trim().is_empty()).map(|w| {
          w.turns.push(Turn { from_user: true, text: std::mem::take(&mut w.input).trim().to_owned() });
          w.waiting = true;
          w.error = None;
          SongRequest::Workshop { context: workshop::context(&self.project), section: w.target.section(&self.project), fields: w.fields.clone(), turns: w.turns.clone(), writer: self.writer }
        });
        if let Some(request) = request {
          self.start(backend, 0, "Working on the section\u{2026}", request);
        }
      },
      Action::WorkshopApply => {
        if let Some(workshop) = self.workshop.take() {
          workshop.target.write(&mut self.project, &workshop.fields);
          // The reference images take a renamed or reworded performer, venue or prop.
          self.project.follow_edited_brief();
          self.touch_now();
        }
      },
      Action::WorkshopClose => self.workshop = None,
      Action::DownloadReference(index, to) => {
        if let Some(picture) = self.project.references.get(index).and_then(|r| r.image.as_ref()) {
          match std::fs::copy(picture, &to) {
            Ok(_) => env.toasts.success(format!("Saved to {}. Edit it, then use the upload button on the same card.", to.display())),
            Err(err) => env.toasts.error(format!("Couldn't save the picture: {err}")),
          }
        }
      },
      Action::SheetFromFace(index) => {
        self.project.set_face_leads(true);
        self.apply(env, Action::References(vec![index]));
      },
      Action::UploadReference(index, file) => {
        // An uploaded head shot is the performer, so the sheet follows it; an uploaded sheet
        // is, so the head shot follows that.
        match self.project.references.get(index).map(|reference| reference.kind) {
          Some(ReferenceKind::Face) => self.project.set_face_leads(true),
          Some(ReferenceKind::Look) => self.project.set_face_leads(false),
          _ => {},
        }
        if let Some(reference) = self.project.references.get_mut(index) {
          reference.version += 1;
          reference.error = None;
          self.start(backend, 1, "Bringing in your picture\u{2026}", SongRequest::ImportReference { dir, index, version: self.project.references[index].version, kind: self.project.references[index].kind, file });
        }
      },
      Action::ChangeReference(index, change) => {
        let pictured = self.project.references.get_mut(index).and_then(|reference| reference.image.clone().map(|picture| (reference, picture)));
        if let Some((reference, picture)) = pictured {
          reference.version += 1;
          reference.error = None;
          let item = ReferenceJob { index, version: reference.version, kind: reference.kind, name: reference.name.clone(), description: reference.description.clone(), change: Some((picture, change)), sheet: None };
          let look = self.project.design().look;
          self.start(backend, 1, "Changing the picture\u{2026}", SongRequest::References { dir, look, aspect: self.project.aspect, pro: self.pro(), items: vec![item] });
        }
      },
      Action::Endings => {
        let Some(brief) = self.project.brief.clone() else {
          return;
        };
        let scenes: Vec<_> = self.project.scenes_without_ending().into_iter().map(|index| (index, &self.project.shots[index])).map(|(index, shot)| (index, shot.slot.section.clone(), shot.slot.seconds(), shot.image.clone(), shot.motion.clone())).collect();
        if !scenes.is_empty() {
          self.start(backend, 0, &format!("Writing ending frames for {} scene{}\u{2026}", scenes.len(), if scenes.len() == 1 { "" } else { "s" }), SongRequest::Endings { brief, scenes, writer: self.writer });
        }
      },
      Action::PlanScene(index) => {
        let (Some(brief), Some(style), Some(shot)) = (self.project.brief.clone(), self.project.visual.clone(), self.project.shots.get(index)) else {
          return;
        };
        let neighbour = |shot: Option<&Shot>| shot.filter(|s| !s.image.trim().is_empty()).map(|s| (s.image.clone(), s.ending.clone()));
        let before = neighbour(index.checked_sub(1).and_then(|i| self.project.shots.get(i)));
        let after = neighbour(self.project.shots.get(index + 1));
        let note = std::mem::take(&mut self.scene_note);
        self.start(backend, 0, &format!("Writing scene {} again\u{2026}", index + 1), SongRequest::PlanScene { brief, style, slot: shot.slot.clone(), before, after, lip_sync: self.project.lip_sync, note, writer: self.writer });
      },
      Action::KeepScene(index) => {
        if let Some(shot) = self.project.shots.get_mut(index) {
          shot.keep("Kept by hand");
          self.save(backend);
        }
      },
      Action::RestoreScene(index, version) => {
        if let Some(shot) = self.project.shots.get_mut(index) {
          shot.restore(version);
          self.project.cut = None;
          self.viewing_scene_version = None;
          self.player = None;
          self.save(backend);
        }
      },
      Action::UsePlan(index) => {
        self.project.use_plan(index);
        self.selected = None;
        self.picked.clear();
        self.viewing_plan = None;
        self.save(backend);
      },
      Action::Reattach => {
        let found = self.project.reattach_media(&dir);
        self.save(backend);
        self.error = Some(if found == 0 { "No pictures in the folder match these scenes.".to_owned() } else { format!("{found} picture{} brought back from the folder.", if found == 1 { "" } else { "s" }) });
      },
      Action::Plan => {
        let (Some(grid), Some(brief), Some(style)) = (&self.project.grid, self.project.brief.clone(), self.project.visual.clone()) else {
          return;
        };
        let mut slots = plan::slots(grid, &self.project.sections, self.project.pace);
        // Each line goes on the scene it is sung over, where listening to the track timed them.
        plan::place_lines(&mut slots, &self.project.lines);
        self.pending_slots = slots.clone();
        self.start(backend, 0, &format!("Writing {} scenes\u{2026}", slots.len()), SongRequest::Plan { brief, style, slots, lip_sync: self.project.lip_sync, writer: self.writer });
      },
      Action::Stills(indices, better, frames) => {
        let quality = self.quality();
        let mut shots = Vec::new();
        for frame in plan_frames(&self.project.shots, &indices, quality, frames) {
          // An ending frame is drawn to match its opening: the one on file, unless the run
          // draws it first (`after`), and for a flowing scene that is the ending before it.
          let on_file = if frame.after.is_some() {
            None
          } else if self.project.flows_from(frame.index) {
            self.project.shots[frame.index - 1].take(quality).end_still.clone()
          } else {
            self.project.shots[frame.index].take(quality).still.clone()
          };
          let shot = &mut self.project.shots[frame.index];
          shot.still_version += 1;
          shot.error = None;
          // The final pass recreates the approved draft frame, when there is one.
          let draft = if frame.ending { &shot.draft.end_still } else { &shot.draft.still };
          let guide = if quality == Quality::Final { draft.clone() } else { None };
          let image = if frame.ending { shot.ending.clone() } else { shot.image.clone() };
          shots.push(StillJob { index: frame.index, version: shot.still_version, image, guide, ending: frame.ending, opening: on_file.filter(|_| frame.ending), after: frame.after, sing: shot.sing });
        }
        if !shots.is_empty() {
          let note = if quality == Quality::Final { "Redrawing stills in final quality\u{2026}" } else { "Drawing stills\u{2026}" };
          self.start(backend, shots.len(), note, SongRequest::Stills { dir, design: self.project.design(), references: self.project.reference_images(), aspect: self.project.aspect, quality, better, shots });
        }
      },
      Action::Clips(indices) => {
        let quality = self.quality();
        let mut shots = Vec::new();
        for index in indices {
          // A scene being lip-synced keeps its clip until that is done.
          if self.sync.as_ref().is_some_and(|run| run.scenes.contains(&index)) {
            continue;
          }
          if let Some(shot) = self.project.shots.get_mut(index) {
            // A scene that asks for an ending frame is filmed once that frame is drawn.
            if let Some(still) = shot.take(quality).still.clone().filter(|_| shot.clip_ready(quality)) {
              shot.clip_version += 1;
              shot.clip_error = None;
              let end_still = shot.end_frame(quality).cloned();
              shots.push(ClipJob { index, version: shot.clip_version, still, end_still, motion: shot.motion.clone(), seconds: shot.slot.seconds(), sing: shot.sing });
            }
          }
        }
        if !shots.is_empty() && self.can_make_clips() {
          // Sung scenes are filmed first: lip sync needs their clips, and can then run on fal
          // while Veo makes the rest.
          shots.sort_by_key(|job| !job.sing);
          let sung_first = shots.first().is_some_and(|job| job.sing) && shots.last().is_some_and(|job| !job.sing);
          let scenes = shots.iter().map(|shot| shot.index).collect();
          self.launch(backend, false, scenes, shots.len(), if sung_first { "Veo is making clips, sung scenes first\u{2026}" } else { "Veo is making clips\u{2026}" }, SongRequest::Clips { dir, look: self.project.design().look, aspect: self.project.aspect, tier: self.tier(), resolution: self.resolution().to_owned(), quality, shots });
        }
      },
      Action::LipSync(indices) => {
        let quality = self.quality();
        let Some(track) = self.project.track.as_ref().map(|t| t.path.clone()) else {
          return;
        };
        let mut shots = Vec::new();
        for index in indices {
          // A scene whose clip is being made again is synced once the new clip is in.
          if self.run.as_ref().is_some_and(|run| run.scenes.contains(&index)) {
            continue;
          }
          if let Some(shot) = self.project.shots.get_mut(index) {
            if let Some(clip) = shot.take(quality).clip.clone() {
              shot.clip_version += 1;
              shot.clip_error = None;
              shots.push(SyncJob { index, version: shot.clip_version, clip, start: shot.slot.start, end: shot.slot.end });
            }
          }
        }
        if !shots.is_empty() && self.can_sync() {
          let scenes = shots.iter().map(|shot| shot.index).collect();
          self.launch(backend, true, scenes, shots.len(), "Lip-syncing sung scenes\u{2026}", SongRequest::LipSync { dir, track, vocals: self.project.vocals.clone(), quality, shots });
        }
      },
      Action::ImportVocals(file) => self.start(backend, 0, "Bringing in the vocal stem\u{2026}", SongRequest::ImportVocals { dir, file }),
      Action::Pass(quality) => {
        if quality != self.project.quality {
          self.project.quality = quality;
          self.player = None;
          self.touch_now();
        }
      },
      Action::UseTake(index) => {
        self.project.use_take(index);
        if !self.project.reachable(self.stage) {
          self.stage = self.project.furthest();
        }
        self.viewing_take = None;
        self.selected = None;
        self.player = None;
        self.save(backend);
      },
      Action::Share => {
        if let Some(film) = self.project.cut.clone() {
          self.start(backend, 0, "Encoding the Instagram copy on your Mac, in two passes\u{2026}", SongRequest::Share { film });
        }
      },
      Action::Cut => {
        if let (Some(track), Some(shots)) = (&self.project.track, self.project.cut_shots(self.quality())) {
          self.project.cut_version += 1;
          self.pending_cut = self.project.cut_media(self.quality()).map(|media| CutOf { quality: self.quality(), media, finish: Some((self.project.grade, self.project.fades)), format: Some(self.project.format) });
          let request = SongRequest::Cut { dir, audio: track.path.clone(), aspect: self.project.aspect, resolution: self.resolution().to_owned(), grade: self.project.grade, fades: self.project.fades, format: self.project.format, shots, version: self.project.cut_version };
          self.start(backend, 0, "Cutting the film on your Mac\u{2026}", request);
        }
      },
      Action::Stop | Action::StopSync => {
        if let Some(run) = self.slot(matches!(action, Action::StopSync)) {
          backend.cancel_song(run.id);
          run.note = "Stopping after the ones in flight\u{2026}".to_owned();
        }
      },
      Action::EditFilm => {
        if let Some(film) = self.film() {
          env.requests.push(AppRequest::EditFilm(Box::new(film)));
        }
      },
      Action::Reveal(path) => {
        if let Err(err) = open::that(path.parent().unwrap_or(&path)) {
          env.toasts.error(format!("Couldn't open the folder: {err}"));
        }
      },
    }
  }

  fn start(&mut self, backend: &Backend, total: usize, note: &str, request: SongRequest) {
    if self.busy() {
      return;
    }
    self.launch(backend, false, BTreeSet::new(), total, note, request);
  }

  /// Starts a run in its slot: the lip sync's, or the main one. `scenes` are the ones whose clip
  /// it makes or syncs. The caller has checked that the slot is free.
  fn launch(&mut self, backend: &Backend, sync: bool, scenes: BTreeSet<usize>, total: usize, note: &str, request: SongRequest) {
    let id = NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed);
    self.error = None;
    let clips = matches!(request, SongRequest::Clips { .. });
    *self.slot(sync) = Some(Run { id, stage: self.stage, note: note.to_owned(), started_as: note.to_owned(), done: 0, total, then_analyse: false, quality: self.project.quality, scenes, clips });
    // The project's folder has to exist before a run writes into it.
    self.save(backend);
    backend.run_song(id, &self.project.id, request);
  }

  fn analyse(&mut self, backend: &Backend) {
    if let Some(track) = &self.project.track {
      let request = SongRequest::Analyse { audio: track.path.clone(), lyrics: self.project.lyrics.clone() };
      self.start(backend, 0, "Finding the beat\u{2026}", request);
    }
  }

  fn analyse_after(&mut self) {
    if let Some(run) = &mut self.run {
      run.then_analyse = true;
    }
  }

  fn go(&mut self, stage: Stage) {
    if stage != self.stage {
      self.player = None;
    }
    self.stage = stage;
    self.project.stage = stage;
    self.touch_now();
  }

  fn progress(&mut self, sync: bool) {
    if let Some(run) = self.slot(sync) {
      run.done += 1;
      run.note.clone_from(&run.started_as);
    }
  }

  /// A fresh number for the song's file name.
  fn next_version(&self) -> u32 {
    (chrono::Utc::now().timestamp() % 1_000_000) as u32
  }

  /// Marks the project changed; it's saved once typing pauses.
  pub(super) fn touch(&mut self, ui: &Ui) {
    self.dirty_since = Some(ui.input(|i| i.time));
  }

  fn touch_now(&mut self) {
    self.dirty_since = Some(0.0);
  }

  fn autosave(&mut self, ui: &Ui, backend: &Backend) {
    let Some(since) = self.dirty_since else {
      return;
    };
    let now = ui.input(|i| i.time);
    if now - since >= SAVE_AFTER_SECS {
      self.save(backend);
    } else {
      ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(SAVE_AFTER_SECS));
    }
  }

  fn save(&mut self, backend: &Backend) {
    self.dirty_since = None;
    // An untouched new project isn't worth a folder.
    let empty = self.project.idea.trim().is_empty() && self.project.lyrics.trim().is_empty() && self.project.track.is_none();
    let Some(library_dir) = backend.library_dir().filter(|_| !empty) else {
      return;
    };
    if let Err(err) = self.project.save(&library_dir) {
      log::warn!("Couldn't save the music video project: {err:#}");
    }
    self.recent = None;
  }
}

/// The reference list's kinds, as headings.
pub(super) fn kind_title(kind: ReferenceKind) -> &'static str {
  match kind {
    ReferenceKind::Look => "Performer",
    ReferenceKind::Face => "Face",
    ReferenceKind::Location => "Location",
    ReferenceKind::Prop => "Prop",
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// The button counts pictures, not scenes: a scene is an opening frame and, unless its ending
  /// is switched off, an ending frame drawn in the same run.
  #[test]
  fn a_run_counts_both_of_a_scenes_frames() {
    let scene = |ending: &str| {
      let mut shot = Shot::default();
      shot.image = "A dock".into();
      shot.ending = ending.into();
      shot
    };
    let mut shots = vec![scene("A face"), scene("A face"), scene(""), Shot::default()];
    shots[1].skip_ending = true;
    let all = [0, 1, 2, 3];
    assert_eq!(frame_count(&shots, &all, Quality::Draft, Frames::Due), (3, 1), "nothing drawn yet: three openings and the one ending that is wanted");
    // With its opening on file, only the ending is left to draw.
    shots[0].set_still(Quality::Draft, PathBuf::from("a.png"));
    assert_eq!(plan_frames(&shots, &[0], Quality::Draft, Frames::Due), [PlannedFrame { index: 0, ending: true, after: None }]);
    shots[0].set_end_still(Quality::Draft, PathBuf::from("a_end.png"));
    assert_eq!(frame_count(&shots, &[0], Quality::Draft, Frames::Due), (0, 0));
    assert_eq!(frame_count(&shots, &all, Quality::Draft, Frames::Both), (3, 1));
    assert_eq!((frame_count(&shots, &[0], Quality::Draft, Frames::Opening), frame_count(&shots, &[0], Quality::Draft, Frames::Ending)), ((1, 0), (0, 1)));
    // The final pass redraws drafts: scene 1 has both, the others have none to redraw.
    assert_eq!(frame_count(&shots, &all, Quality::Final, Frames::Due), (1, 1));
  }

  /// A flowing scene has no opening to draw: it opens on the ending before it, and its own
  /// ending waits for that one when the same run draws it.
  #[test]
  fn a_flowing_scene_draws_only_its_ending() {
    let scene = |flows: bool| {
      let mut shot = Shot::default();
      shot.image = "A dock".into();
      shot.ending = "A face".into();
      shot.flows = flows;
      shot
    };
    let mut shots = vec![scene(false), scene(true), scene(true)];
    let frame = |index, ending, after| PlannedFrame { index, ending, after };
    assert_eq!(plan_frames(&shots, &[2, 0, 1], Quality::Draft, Frames::Due), [frame(0, false, None), frame(0, true, Some((0, false))), frame(1, true, Some((0, true))), frame(2, true, Some((1, true)))]);
    assert_eq!(frame_count(&shots, &[0, 1, 2], Quality::Draft, Frames::Both), (1, 3));
    // On its own, a flowing scene can be drawn once the ending before it is on file.
    assert_eq!(plan_frames(&shots, &[1], Quality::Draft, Frames::Both), []);
    shots[0].set_end_still(Quality::Draft, PathBuf::from("a_end.png"));
    assert_eq!(plan_frames(&shots, &[1], Quality::Draft, Frames::Both), [frame(1, true, None)]);
    assert_eq!(plan_frames(&shots, &[1], Quality::Draft, Frames::Opening), []);
  }
}
