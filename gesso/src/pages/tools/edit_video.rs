//! Edit Video: trim a clip and join clips on the user's Mac (FFmpeg, free), extend a clip by
//! about 7 seconds with Veo, or describe a change ("remove the dog's tail") for Gemini Omni Flash
//! to make, both on Vertex.
//!
//! A film sent over from the Music Video page opens as its parts: the scenes in order, each with
//! its length, over the soundtrack. Scenes can be reordered, removed or given another clip
//! (one made here with the other tabs included), and the film cut again on the user's Mac.

use std::path::PathBuf;

use egui::{Rect, Ui};

use super::{Source, ToolSession};
use crate::backend::local::tools::{Film, OMNI_LONGEST_CLIP, OMNI_MAX_SECONDS, OMNI_RESOLUTIONS, ToolRequest, VeoTier, omni_parts};
use crate::backend::video::{self, VideoPlayer};
use crate::pages::common::{Env, pick_files};
use crate::prompt_box::refs::RefKind;
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets;

const ACCEPTS: [RefKind; 1] = [RefKind::Video];
/// What a scene of a film can be replaced with, and a film's soundtrack.
const SCENE_ACCEPTS: [RefKind; 2] = [RefKind::Video, RefKind::Image];
const SOUND_ACCEPTS: [RefKind; 1] = [RefKind::Audio];
/// The height of the scene list before it scrolls.
const SCENE_LIST_HEIGHT: f32 = 168.0;
const TIERS: [VeoTier; 3] = [VeoTier::Fast, VeoTier::Standard, VeoTier::Lite];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
  /// A film as its scenes and soundtrack (offered only when one is open).
  Film,
  #[default]
  Trim,
  Join,
  Extend,
  Describe,
}

/// What a click in the film's scene list asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SceneAction {
  Pick,
  Remove,
  Earlier,
  Later,
}

#[derive(Default)]
pub struct EditVideoPage {
  pub session: ToolSession,
  mode: Mode,
  player: Option<VideoPlayer>,
  start: Option<f64>,
  end: Option<f64>,
  /// Clips to join, in order (the source is added first).
  clips: Vec<Source>,
  prompt: String,
  tier: usize,
  /// The Describe tab's change, resolution (index into `OMNI_RESOLUTIONS`) and sound choice.
  change: String,
  resolution: usize,
  replace_sound: bool,
  /// The film being edited as its parts, and the scene picked in its list.
  film: Option<Film>,
  scene: usize,
}

impl EditVideoPage {
  pub fn open(&mut self, path: PathBuf) {
    let source = Source::from_path(path);
    if !self.clips.contains(&source) {
      self.clips.push(source.clone());
    }
    self.session.source = Some(source);
    self.player = None;
    self.start = None;
    self.end = None;
    self.session.error = None;
  }

  /// Opens a film as its scenes and soundtrack, on the Film tab, with its first scene on screen.
  pub fn open_film(&mut self, film: Film) {
    self.mode = Mode::Film;
    self.scene = 0;
    self.film = Some(film);
    self.show_scene();
  }

  /// Puts the picked scene's clip on screen (a held still has nothing to play).
  fn show_scene(&mut self) {
    let Some(scene) = self.film.as_ref().and_then(|film| film.scenes.get(self.scene)) else {
      return;
    };
    if !scene.still {
      self.show(scene.media.clone());
    }
  }

  /// Puts a clip on screen without adding it to the Join list.
  fn show(&mut self, path: PathBuf) {
    self.session.source = Some(Source::from_path(path));
    self.player = None;
    self.start = None;
    self.end = None;
    self.session.error = None;
  }

  pub fn ui(&mut self, ui: &mut Ui, env: &mut Env<'_>) {
    let (stage, panel, strip) = super::layout(ui.max_rect());
    match self.session.source.clone() {
      None => {
        if let Some(path) = super::empty_stage(ui, stage, &ACCEPTS, "a video") {
          self.open(path);
        }
      },
      Some(source) => {
        ui.painter().rect_filled(stage, theme::RADIUS, theme::SUNKEN);
        let player = self.player.get_or_insert_with(|| VideoPlayer::new(&source.url));
        player.ui(ui, stage.shrink(12.0).with_min_y(stage.top() + 48.0), None);
        if let Some(path) = super::source_bar(ui, stage, &source, &ACCEPTS) {
          self.open(path);
        }
        if self.session.busy() {
          let message = match self.mode {
            Mode::Extend => "Veo is extending the clip (about a minute)\u{2026}",
            Mode::Describe => "Gemini Omni Flash is editing the clip (a few minutes)\u{2026}",
            _ => "Working\u{2026}",
          };
          super::busy_overlay(ui, stage, message);
        }
      },
    }
    if !video::ffmpeg_available() {
      self.session.error = Some("Edit Video needs FFmpeg. Install it with `brew install ffmpeg`, then restart the app.".to_owned());
    }
    self.panel(ui, panel, env);
    if let Some(picked) = super::results_strip(ui, strip, &self.session.results, env.cache, "Edited clips appear here. Click one to keep working on it.") {
      self.open(picked.path);
    }
  }

  /// The Film tab: the soundtrack, the scenes in order, what can be done to the picked scene, and
  /// the cut. Returns the cut to run when its button is pressed.
  fn film_tab(&mut self, ui: &mut Ui) -> Option<ToolRequest> {
    let on_screen = self.session.source.as_ref().map(|source| source.path.clone());
    let film = self.film.as_mut()?;
    let mut show = false;
    ui.label(egui::RichText::new(&film.title).size(14.0).family(theme::semibold()));
    ui.label(egui::RichText::new(format!("{} scenes \u{b7} {:.1} s \u{b7} {}\u{d7}{}", film.scenes.len(), film.seconds(), film.size.0, film.size.1)).size(12.0).family(theme::mono()).color(theme::MUTED));

    super::field(ui, "Soundtrack");
    ui.horizontal(|ui| {
      let name = film.audio.as_ref().and_then(|path| path.file_name()).map_or_else(|| "None".to_owned(), |name| name.to_string_lossy().into_owned());
      ui.label(egui::RichText::new(name).size(12.5));
      ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if widgets::ghost(ui, Icon::Upload, None, "Use another soundtrack. It runs under every scene from the start, so it stays in place when scenes change", false).clicked() {
          if let Some(path) = pick_files(&SOUND_ACCEPTS, false).into_iter().next() {
            film.audio = Some(path);
          }
        }
      });
    });

    super::field(ui, "Scenes, in order");
    let mut action = None;
    egui::ScrollArea::vertical().id_salt("film-scenes").max_height(SCENE_LIST_HEIGHT).auto_shrink([false, true]).show(ui, |ui| {
      let mut at = 0.0;
      for (i, scene) in film.scenes.iter().enumerate() {
        let picked = i == self.scene;
        let start = at;
        at += scene.seconds;
        ui.horizontal(|ui| {
          let text = format!("{} \u{b7} {:.1}s at {:.1}s{}", scene.label, scene.seconds, start, if scene.still { " \u{b7} still" } else { "" });
          let label = egui::RichText::new(text).size(12.5).color(if picked { theme::INK } else { theme::MUTED });
          let label = if picked { label.family(theme::semibold()) } else { label };
          if ui.add(egui::Button::new(label).frame(false)).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(if scene.lyric.is_empty() { "Pick this scene".to_owned() } else { format!("Sung: {}", scene.lyric) }).clicked() {
            action = Some((i, SceneAction::Pick));
          }
          ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::ghost(ui, Icon::X, None, "Remove this scene; the ones after it move earlier", false).clicked() {
              action = Some((i, SceneAction::Remove));
            }
            if i + 1 < film.scenes.len() && widgets::ghost(ui, Icon::ChevronDown, None, "Move later", false).clicked() {
              action = Some((i, SceneAction::Later));
            }
            if i > 0 && widgets::ghost(ui, Icon::ChevronUp, None, "Move earlier", false).clicked() {
              action = Some((i, SceneAction::Earlier));
            }
          });
        });
      }
    });
    match action {
      Some((i, SceneAction::Pick)) => {
        self.scene = i;
        show = true;
      },
      Some((i, SceneAction::Remove)) => {
        film.scenes.remove(i);
        self.scene = self.scene.min(film.scenes.len().saturating_sub(1));
        show = true;
      },
      Some((i, SceneAction::Earlier)) => {
        film.scenes.swap(i, i - 1);
        self.scene = i - 1;
      },
      Some((i, SceneAction::Later)) => {
        film.scenes.swap(i, i + 1);
        self.scene = i + 1;
      },
      None => {},
    }

    if let Some(scene) = film.scenes.get_mut(self.scene) {
      super::field(ui, &format!("Scene {}", scene.label));
      ui.horizontal_wrapped(|ui| {
        if widgets::button(ui, Some(Icon::Upload), "Replace with a file", widgets::Kind::Secondary, 0.0, 30.0).clicked() {
          if let Some(path) = pick_files(&SCENE_ACCEPTS, false).into_iter().next() {
            scene.still = Source::from_path(path.clone()).kind != crate::feed::types::MediaKind::Video;
            scene.media = path;
            show = true;
          }
        }
        // A clip made on the other tabs (trimmed, extended, changed) and picked from the strip below.
        let other = on_screen.as_ref().filter(|path| **path != scene.media);
        if ui.add_enabled_ui(other.is_some(), |ui| widgets::button(ui, None, "Use the clip on screen", widgets::Kind::Secondary, 0.0, 30.0).on_hover_text("Puts the clip now on screen into this scene: one you made on the Trim, Extend or Describe tab and picked from the strip below")).inner.clicked() {
          if let Some(path) = other {
            scene.media.clone_from(path);
            scene.still = false;
          }
        }
      });
      ui.label(egui::RichText::new("The scene keeps its length. To change its clip, use Trim, Extend or Describe, pick the result from the strip below, then use it here.").size(12.0).color(theme::FAINT));
    }
    ui.add_space(6.0);
    let ready = film.audio.is_some() && !film.scenes.is_empty();
    let request = super::run_button(ui, "Cut the film", Some(0.0), ready, &self.session).then(|| ToolRequest::CutFilm { film: film.clone() });
    if show {
      self.show_scene();
    }
    request
  }

  fn panel(&mut self, ui: &mut Ui, rect: Rect, env: &mut Env<'_>) {
    let position = self.player.as_ref().map_or(0.0, VideoPlayer::position);
    let duration = self.player.as_ref().and_then(VideoPlayer::duration);
    let has_source = self.session.source.is_some();
    let extend_cost = env.backend.estimate_tool(&ToolRequest::ExtendVideo { video: PathBuf::new(), prompt: String::new(), tier: TIERS[self.tier] });
    let resolution = OMNI_RESOLUTIONS.get(self.resolution).copied().unwrap_or("720p");
    let describe_cost = duration.and_then(|seconds| env.backend.estimate_tool(&ToolRequest::DescribeEdit { video: PathBuf::new(), prompt: String::new(), seconds, resolution: resolution.to_owned(), keep_sound: true }));
    let mut run = None;
    super::panel(ui, rect, "Edit Video", "Trim and join clips on your Mac, have Veo continue a clip, or describe a change.", |ui| {
      // The Film tab is there only while a film is open.
      let (modes, labels): (Vec<Mode>, Vec<&str>) = if self.film.is_some() { (vec![Mode::Film, Mode::Trim, Mode::Join, Mode::Extend, Mode::Describe], vec!["Film", "Trim", "Join", "Extend", "Describe"]) } else { (vec![Mode::Trim, Mode::Join, Mode::Extend, Mode::Describe], vec!["Trim", "Join", "Extend", "Describe"]) };
      if let Some(i) = super::segmented(ui, &labels, modes.iter().position(|m| *m == self.mode).unwrap_or(0)) {
        self.mode = modes[i];
      }
      match self.mode {
        Mode::Film => run = self.film_tab(ui),
        Mode::Trim => {
          super::field(ui, "Keep from \u{2192} to");
          ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(has_source, |ui| {
              if widgets::button(ui, None, &format!("Start here ({position:.2}s)"), widgets::Kind::Secondary, 0.0, 30.0).clicked() {
                self.start = Some(position);
              }
              if widgets::button(ui, None, &format!("End here ({position:.2}s)"), widgets::Kind::Secondary, 0.0, 30.0).clicked() {
                self.end = Some(position);
              }
            });
          });
          let (start, end) = (self.start.unwrap_or(0.0), self.end.or(duration).unwrap_or(0.0));
          ui.label(egui::RichText::new(format!("{start:.2}s \u{2192} {end:.2}s  ({:.2}s)", (end - start).max(0.0))).size(13.0).family(theme::mono()));
          if super::run_button(ui, "Trim clip", Some(0.0), has_source && end > start + 0.05, &self.session) {
            if let Some(source) = &self.session.source {
              run = Some(ToolRequest::TrimVideo { video: source.path.clone(), start, end });
            }
          }
        },
        Mode::Join => {
          super::field(ui, "Clips, in order");
          let mut action = None;
          for (i, clip) in self.clips.iter().enumerate() {
            ui.horizontal(|ui| {
              ui.label(egui::RichText::new(format!("{}. {}", i + 1, clip.name())).size(12.5));
              ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::ghost(ui, Icon::X, None, "Remove", false).clicked() {
                  action = Some((i, 0));
                }
                if i > 0 && widgets::ghost(ui, Icon::ChevronUp, None, "Move up", false).clicked() {
                  action = Some((i, 1));
                }
              });
            });
          }
          match action {
            Some((i, 0)) => {
              self.clips.remove(i);
            },
            Some((i, _)) => self.clips.swap(i, i - 1),
            None => {},
          }
          ui.add_space(6.0);
          if widgets::button(ui, Some(Icon::Plus), "Add clips", widgets::Kind::Secondary, 0.0, 30.0).clicked() {
            for path in pick_files(&ACCEPTS, true) {
              self.clips.push(Source::from_path(path));
            }
          }
          ui.label(egui::RichText::new("Clips are scaled to the first one's size. Sound is kept when every clip has it.").size(12.0).color(theme::FAINT));
          if super::run_button(ui, &format!("Join {} clips", self.clips.len()), Some(0.0), self.clips.len() >= 2, &self.session) {
            run = Some(ToolRequest::JoinVideos { videos: self.clips.iter().map(|c| c.path.clone()).collect() });
          }
        },
        Mode::Extend => {
          super::field(ui, "What happens next");
          super::prompt_field(ui, "extend-prompt", &mut self.prompt, "The camera keeps tracking her as she walks into the market");
          super::field(ui, "Model");
          if let Some(i) = super::segmented(ui, &TIERS.map(VeoTier::label), self.tier) {
            self.tier = i;
          }
          ui.label(egui::RichText::new("Adds about 7 seconds. Clips up to 30 s; they're converted to 24 fps 720p first.").size(12.0).color(theme::FAINT));
          if super::run_button(ui, "Extend clip", extend_cost, has_source && !self.prompt.trim().is_empty(), &self.session) {
            if let Some(source) = &self.session.source {
              run = Some(ToolRequest::ExtendVideo { video: source.path.clone(), prompt: self.prompt.trim().to_owned(), tier: TIERS[self.tier] });
            }
          }
        },
        Mode::Describe => {
          super::field(ui, "The change");
          super::prompt_field(ui, "describe-prompt", &mut self.change, "Remove the dog's tail so it has a natural bobtail");
          super::field(ui, "Resolution");
          if let Some(i) = super::segmented(ui, &OMNI_RESOLUTIONS, self.resolution.min(OMNI_RESOLUTIONS.len() - 1)) {
            self.resolution = i;
          }
          let mut keep = !self.replace_sound;
          ui.checkbox(&mut keep, "Keep the clip's own sound");
          self.replace_sound = !keep;
          let parts = duration.map_or(0, |d| omni_parts(d).len());
          let note = match duration {
            Some(d) if d > OMNI_LONGEST_CLIP + 0.5 => format!("This clip is {d:.0} s; Describe edits take up to {OMNI_LONGEST_CLIP:.0} s. Trim it first."),
            Some(_) if parts > 1 => format!("Edited in {parts} parts of up to {OMNI_MAX_SECONDS:.0} s, then joined. Each part is redrawn, so details can shift where parts meet. Comes back at 24 fps."),
            _ => "The whole clip is redrawn with your change, at 24 fps. Fur and background can shift slightly.".to_owned(),
          };
          ui.label(egui::RichText::new(note).size(12.0).color(theme::FAINT));
          let fits = duration.is_some_and(|d| d <= OMNI_LONGEST_CLIP + 0.5);
          if super::run_button(ui, "Edit clip", describe_cost, has_source && fits && !self.change.trim().is_empty(), &self.session) {
            if let (Some(source), Some(seconds)) = (&self.session.source, duration) {
              run = Some(ToolRequest::DescribeEdit { video: source.path.clone(), prompt: self.change.trim().to_owned(), seconds, resolution: resolution.to_owned(), keep_sound: !self.replace_sound });
            }
          }
        },
      }
    });
    if let Some(request) = run {
      self.session.run(env, request);
    }
  }
}
