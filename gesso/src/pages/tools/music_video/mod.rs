//! Music Video: one staged workflow from an idea to a finished film. The stages run down the left
//! rail (Idea, Lyrics, Song, Music Analysis, Visual Style, Creative Brief, Reference Images,
//! Scenes, Shots, Final Video); each opens once the one before it has produced what it needs, and
//! everything a stage makes can be edited before the next one uses it.
//!
//! The project is saved after every change (`~/Artcraft/local/songs/<id>/project.json`), so a
//! video can be picked up later from the list on the first stage.

mod stages;
mod workshop;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use egui::{Align2, FontId, Rect, Sense, Ui, pos2, vec2};

use crate::backend::Backend;
use crate::backend::local::song::plan::{self, Slot};
use crate::backend::local::song::project::{CutOf, Quality, ReferenceKind, SongProject, Stage, Summary, Track};
use crate::backend::local::song::{ClipJob, ReferenceJob, SongRequest, SongUpdate, StillJob, SyncJob, Writer};
use crate::backend::local::song::writer::Turn;
use crate::backend::local::tools::VeoTier;
use crate::backend::video::VideoPlayer;
use crate::pages::common::Env;
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
  Plan,
  /// Scenes whose stills to draw, by index; `true` to use the better model in the draft pass.
  Stills(Vec<usize>, bool),
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

/// Something for the app to say after an update.
pub struct Notice {
  pub error: bool,
  pub message: String,
  /// The library gained a file (the song, a reference image or the finished film).
  pub library_changed: bool,
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
  /// A 30-second clip instead of a full song.
  short_song: bool,
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
}

impl Default for MusicVideoPage {
  fn default() -> Self {
    Self { project: SongProject::new(), stage: Stage::Idea, run: None, sync: None, error: None, analysis_note: None, recent: None, writer: Writer::default(), notes: String::new(), short_song: false, instrumental: false, tier: DEFAULT_TIERS, resolution: DEFAULT_RESOLUTIONS, selected: None, better_redraw: false, picked: BTreeSet::new(), player: None, workshop: None, reference_changes: Vec::new(), pending_slots: Vec::new(), pending_cut: None, dirty_since: None, open_on_start: None }
  }
}

impl MusicVideoPage {
  /// The page as the app starts: `ARTCRAFT_MUSIC_VIDEO=<project id>` opens that saved project
  /// (with `ARTCRAFT_PAGE=music-video`, for screenshots).
  pub fn from_env() -> Self {
    Self { open_on_start: std::env::var(OPEN_ENV).ok().filter(|id| !id.is_empty()), ..Self::default() }
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
    }
    let mut actions = Vec::new();
    self.rail(ui, rail, &mut actions);
    stages::show(self, ui, env, stage, panel, &mut actions);
    workshop::show(self, ui, &mut actions);
    for action in actions {
      self.apply(env, action);
    }
    self.autosave(ui, env.backend);
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
        self.project.track = Some(Track { path, model, sung });
        self.project.grid = None;
        self.project.sections.clear();
        self.project.lines.clear();
        self.project.shots.clear();
        self.project.vocals = None;
        self.project.cut = None;
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
      SongUpdate::Planned(scenes) => {
        let slots = std::mem::take(&mut self.pending_slots);
        self.project.set_plan(&slots, &scenes);
        self.selected = None;
        self.picked.clear();
      },
      SongUpdate::Still { index, path } => {
        if let Some(shot) = self.project.shots.get_mut(index) {
          // The clip (and its lip sync) was made from the old still.
          let take = shot.take_mut(quality);
          self.project.in_library.insert(path.clone());
          take.still = Some(path);
          take.clip = None;
          take.synced = None;
          shot.error = None;
          shot.clip_error = None;
        }
        self.progress(sync);
      },
      SongUpdate::Clip { index, path } => {
        if let Some(shot) = self.project.shots.get_mut(index) {
          // A lip sync of the old clip no longer applies.
          let take = shot.take_mut(quality);
          self.project.in_library.insert(path.clone());
          take.clip = Some(path);
          take.synced = None;
          shot.clip_error = None;
        }
        if self.selected == Some(index) {
          self.player = None;
        }
        self.progress(sync);
      },
      SongUpdate::Synced { index, path } => {
        if let Some(shot) = self.project.shots.get_mut(index) {
          self.project.in_library.insert(path.clone());
          shot.take_mut(quality).synced = Some(path);
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

  /// The left rail: the project's name and the stages, each with its state.
  fn rail(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
    ui.painter().rect(rect, theme::RADIUS, theme::PANEL, theme::hairline(), egui::StrokeKind::Inside);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(12.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut child;
    widgets::hud(ui, "Music video", theme::FAINT);
    ui.add_space(4.0);
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
  }

  fn apply(&mut self, env: &mut Env<'_>, action: Action) {
    let backend = env.backend;
    let Some(library_dir) = backend.library_dir() else {
      return;
    };
    let dir = self.project.dir(&library_dir);
    match action {
      Action::Go(stage) => self.go(stage),
      Action::New => {
        self.save(backend);
        *self = Self { writer: self.writer, tier: self.tier, resolution: self.resolution, ..Self::default() };
      },
      Action::Open(id) => {
        self.save(backend);
        if let Some(project) = SongProject::load(&library_dir, &id) {
          let stage = if project.reachable(project.stage) { project.stage } else { project.furthest() };
          *self = Self { project, stage, writer: self.writer, tier: self.tier, resolution: self.resolution, ..Self::default() };
          // Everything the video has made belongs in the library, whenever it was made.
          let request = SongRequest::Catalogue { dir: self.project.dir(&library_dir), known: self.project.in_library.clone() };
          self.start(backend, 0, "Checking this video's files are in the library\u{2026}", request);
        }
      },
      Action::Write { rewrite } => {
        let current = rewrite.then(|| self.project.draft());
        let note = if rewrite { "Rewriting the song\u{2026}" } else { "Writing the song\u{2026}" };
        self.start(backend, 0, note, SongRequest::Write { idea: self.project.idea.clone(), notes: self.notes.clone(), current, writer: self.writer });
      },
      Action::OwnLyrics => {
        if self.project.lyrics.trim().is_empty() {
          self.project.lyrics = "[Verse 1]\n".to_owned();
        }
        self.go(Stage::Lyrics);
      },
      Action::Compose => {
        let version = self.next_version();
        self.start(backend, 0, "Lyria is making the song (a minute or two)\u{2026}", SongRequest::Compose { dir, draft: self.project.draft(), clip: self.short_song, instrumental: self.instrumental, version });
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
        for index in indices {
          if let Some(reference) = self.project.references.get_mut(index) {
            if reference.kind == ReferenceKind::Face && sheet_in_this_run {
              continue;
            }
            reference.version += 1;
            reference.error = None;
            let sheet = sheet_picture.clone().filter(|_| reference.kind == ReferenceKind::Face);
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
      Action::UploadReference(index, file) => {
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
      Action::Stills(indices, better) => {
        let quality = self.quality();
        let mut shots = Vec::new();
        for index in indices {
          if let Some(shot) = self.project.shots.get_mut(index).filter(|s| !s.image.trim().is_empty()) {
            shot.still_version += 1;
            shot.error = None;
            // The final pass recreates the approved draft frame, when there is one.
            let guide = if quality == Quality::Final { shot.draft.still.clone() } else { None };
            shots.push(StillJob { index, version: shot.still_version, image: shot.image.clone(), guide });
          }
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
            if let Some(still) = shot.take(quality).still.clone() {
              shot.clip_version += 1;
              shot.clip_error = None;
              shots.push(ClipJob { index, version: shot.clip_version, still, motion: shot.motion.clone(), seconds: shot.slot.seconds(), sing: shot.sing });
            }
          }
        }
        if !shots.is_empty() && self.can_make_clips() {
          let scenes = shots.iter().map(|shot| shot.index).collect();
          self.launch(backend, false, scenes, shots.len(), "Veo is making clips\u{2026}", SongRequest::Clips { dir, look: self.project.design().look, aspect: self.project.aspect, tier: self.tier(), resolution: self.resolution().to_owned(), quality, shots });
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
      Action::Cut => {
        if let (Some(track), Some(shots)) = (&self.project.track, self.project.cut_shots(self.quality())) {
          self.project.cut_version += 1;
          self.pending_cut = self.project.cut_media(self.quality()).map(|media| CutOf { quality: self.quality(), media });
          let request = SongRequest::Cut { dir, audio: track.path.clone(), aspect: self.project.aspect, resolution: self.resolution().to_owned(), grade: self.project.grade, shots, version: self.project.cut_version };
          self.start(backend, 0, "Cutting the film on your Mac\u{2026}", request);
        }
      },
      Action::Stop | Action::StopSync => {
        if let Some(run) = self.slot(matches!(action, Action::StopSync)) {
          backend.cancel_song(run.id);
          run.note = "Stopping after the ones in flight\u{2026}".to_owned();
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
    backend.run_song(id, request);
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
