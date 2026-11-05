//! What each stage of the Music Video page shows: its work in the middle column, and its
//! settings, run button and cost in the options panel.

use std::path::{Path, PathBuf};

use egui::{Align2, Color32, FontId, Rect, Sense, Ui, pos2, vec2};

use super::workshop::Target;
use super::{Action, MusicVideoPage, RESOLUTIONS, TIERS, kind_title};
use crate::backend::local::library::file_url;
use crate::backend::local::song::cut::{Aspect, Grade};
use crate::backend::local::song::plan::{self, Energy, Pace};
use crate::backend::local::song::project::{Quality, Stage};
use crate::backend::local::song::writer::{Subject, Swatch};
use crate::backend::local::song::{SongEstimate, Writer, thumb_path};
use crate::backend::media_cache::{Lookup, MediaCache};
use crate::backend::video::VideoPlayer;
use crate::overlays::lightbox::fit_inside;
use crate::pages::common::{AppRequest, Env, pick_files};
use crate::pages::tools::{field, panel, prompt_field, segmented};
use crate::prompt_box::refs::RefKind;
use crate::shell::usd;
use crate::theme;
use crate::ui::icons::{self, Icon};
use crate::ui::waveform::{self, format_time};
use crate::ui::widgets;

const CARD_GAP: f32 = 12.0;
/// Scene and reference cards are at least this wide; the grid fits as many columns as it can.
const CARD_MIN_WIDTH: f32 = 210.0;
const CARD_TEXT_HEIGHT: f32 = 54.0;
const TIMELINE_HEIGHT: f32 = 28.0;
/// Reference cards are bigger: there are only a handful, and they're looked at closely.
const REFERENCE_MIN_WIDTH: f32 = 240.0;
/// What the writer's calls cost, near enough: a few thousand tokens each.
const CENTS: &str = "< 1\u{a2}";
const A_CENT: &str = "\u{2248} 1\u{a2}";
/// Lyrics and briefs read best in a column, not the full width of a wide window.
const TEXT_COLUMN: f32 = 760.0;

pub(super) fn show(page: &mut MusicVideoPage, ui: &mut Ui, env: &mut Env<'_>, stage: Rect, panel_rect: Rect, actions: &mut Vec<Action>) {
  ui.painter().rect(stage, theme::RADIUS, theme::fade(theme::INK, 0.012), theme::hairline(), egui::StrokeKind::Inside);
  match page.stage {
    Stage::Idea => idea(page, ui, stage, panel_rect, actions),
    Stage::Lyrics => lyrics(page, ui, stage, panel_rect, actions),
    Stage::Song => song(page, ui, env, stage, panel_rect, actions),
    Stage::Analysis => analysis(page, ui, env, stage, panel_rect, actions),
    Stage::Style => style(page, ui, stage, panel_rect, actions),
    Stage::Brief => brief(page, ui, stage, panel_rect, actions),
    Stage::References => references(page, ui, env, stage, panel_rect, actions),
    Stage::Scenes => scenes(page, ui, env, stage, panel_rect, actions),
    Stage::Shots => shots(page, ui, env, stage, panel_rect, actions),
    Stage::Final => final_video(page, ui, env, stage, panel_rect, actions),
  }
}

// --- Idea ------------------------------------------------------------------------------------

fn idea(page: &mut MusicVideoPage, ui: &mut Ui, stage: Rect, panel_rect: Rect, actions: &mut Vec<Action>) {
  let busy = page.busy();
  let mut changed = false;
  column(ui, stage, "idea", |ui| {
    heading(ui, Stage::Idea, "What's the song?", "Describe the song you want: what it's about, how it should feel, the kind of music. The writer turns it into lyrics and a music direction you can edit.");
    changed |= text_box(ui, "mv-idea", &mut page.project.idea, "A snarling big-beat anthem about the people who write prompts and wrangle agents all day\u{2026}", 6);
    ui.add_space(18.0);
    widgets::hud(ui, "Your videos", theme::FAINT);
    ui.add_space(6.0);
    let others: Vec<_> = page.recent.iter().flatten().filter(|s| s.id != page.project.id).cloned().collect();
    if others.is_empty() {
      ui.label(egui::RichText::new("Videos you start are saved as you go, and your other ones are listed here to come back to.").size(13.0).color(theme::FAINT));
    }
    for summary in &others {
      let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), if busy { Sense::hover() } else { Sense::click() });
      ui.painter().rect(row, theme::RADIUS, if resp.hovered() && !busy { theme::WASH_HOVER } else { theme::WASH }, theme::hairline(), egui::StrokeKind::Inside);
      ui.painter().text(pos2(row.left() + 14.0, row.center().y), Align2::LEFT_CENTER, &summary.title, FontId::new(14.0, theme::medium()), theme::INK);
      let detail = format!("{} \u{b7} {}", summary.stage.label(), day(&summary.updated_at));
      ui.painter().text(pos2(row.right() - 14.0, row.center().y), Align2::RIGHT_CENTER, detail, FontId::new(11.5, theme::mono()), theme::MUTED);
      if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !busy {
        actions.push(Action::Open(summary.id.clone()));
      }
      ui.add_space(6.0);
    }
  });
  if changed {
    page.touch(ui);
  }
  let has_idea = !page.project.idea.trim().is_empty();
  let has_lyrics = !page.project.lyrics.trim().is_empty();
  let line = page.progress_line().filter(|_| page.running_here());
  panel(ui, panel_rect, "Idea", "Start from an idea, or bring your own lyrics.", |ui| {
    writer_choice(ui, &mut page.writer);
    error_line(ui, page.error.as_deref());
    if run_button(ui, Icon::Sparkles, if has_lyrics { "Write it again" } else { "Write the song" }, Some(CENTS.to_owned()), has_idea && !busy, line.as_deref()) {
      actions.push(Action::Write { rewrite: false });
    }
    ui.add_space(10.0);
    if secondary(ui, Icon::Pencil, if has_lyrics { "Go to the lyrics" } else { "Write my own lyrics" }, !busy) {
      actions.push(Action::OwnLyrics);
    }
  });
}

// --- Lyrics ----------------------------------------------------------------------------------

fn lyrics(page: &mut MusicVideoPage, ui: &mut Ui, stage: Rect, panel_rect: Rect, actions: &mut Vec<Action>) {
  let busy = page.busy();
  let mut changed = false;
  column(ui, stage, "lyrics", |ui| {
    heading(ui, Stage::Lyrics, "Lyrics", "Edit anything. Section tags in square brackets, like [Verse 1] and [Chorus], tell the music model the song's shape; a tag with no lines under it is instrumental.");
    ui.label(label("Title"));
    changed |= ui.add(egui::TextEdit::singleline(&mut page.project.title).desired_width(f32::INFINITY).font(FontId::new(20.0, theme::display())).margin(egui::Margin::symmetric(10, 8))).changed();
    ui.add_space(12.0);
    ui.label(label("Lyrics"));
    let rows = page.project.lyrics.lines().count().max(16) + 2;
    changed |= text_box(ui, "mv-lyrics", &mut page.project.lyrics, "[Verse 1]\n\u{2026}", rows);
  });
  let has_lyrics = !page.project.lyrics.trim().is_empty();
  let line = page.progress_line().filter(|_| page.running_here());
  panel(ui, panel_rect, "Lyrics", "The words and the music direction the song is made from.", |ui| {
    field(ui, "Music direction");
    let before = page.project.style.clone();
    prompt_field(ui, "mv-style", &mut page.project.style, "Genre and era, tempo, instruments, how the vocal is delivered. No artist or song names: the music model refuses them.");
    changed |= before != page.project.style;
    field(ui, "Change something");
    prompt_field(ui, "mv-notes", &mut page.notes, "Make the chorus shorter and angrier\u{2026}");
    writer_choice(ui, &mut page.writer);
    error_line(ui, page.error.as_deref());
    if run_button(ui, Icon::RotateCw, "Rewrite", Some(CENTS.to_owned()), has_lyrics && !page.notes.trim().is_empty() && !busy, line.as_deref()) {
      actions.push(Action::Write { rewrite: true });
    }
    next_button(ui, Stage::Song, has_lyrics, actions);
  });
  if changed {
    page.touch(ui);
  }
}

// --- Song ------------------------------------------------------------------------------------

fn song(page: &mut MusicVideoPage, ui: &mut Ui, env: &mut Env<'_>, stage: Rect, panel_rect: Rect, actions: &mut Vec<Action>) {
  let busy = page.busy();
  column(ui, stage, "song", |ui| {
    heading(ui, Stage::Song, "Song", "Lyria sings the lyrics to your music direction. Or bring a track you made elsewhere: the video stages work the same either way.");
    match &page.project.track {
      Some(track) => {
        card(ui, if track.model.is_empty() { "Your track".to_owned() } else { format!("Made by {}", track.model) }, |ui| {
          waveform::player(ui, ui.id().with("mv-song"), env.audio, &file_url(&track.path), None, false);
        });
        if !track.sung.trim().is_empty() {
          card(ui, "What the model says it sang".to_owned(), |ui| {
            ui.label(body(&track.sung));
          });
        }
      },
      None => empty(ui, Icon::Music, "No song yet", "Generate one, or import a track."),
    }
  });
  let cost = env.backend.estimate_song(&SongEstimate::Song { clip: page.short_song });
  let line = page.progress_line().filter(|_| page.running_here());
  let has_track = page.project.track.is_some();
  panel(ui, panel_rect, "Song", "Each generation is a new take; the latest one is kept.", |ui| {
    field(ui, "Length");
    if let Some(i) = segmented(ui, &["Full song", "30 s clip"], usize::from(page.short_song)) {
      page.short_song = i == 1;
    }
    ui.add_space(6.0);
    ui.checkbox(&mut page.instrumental, "Instrumental (no vocals)");
    ui.label(small("Lyria 3 on your Vertex AI account. A new song clears the scenes, which are timed to the old one."));
    error_line(ui, page.error.as_deref());
    if run_button(ui, Icon::Sparkles, if has_track { "Make another take" } else { "Make the song" }, cost.map(estimate), !busy, line.as_deref()) {
      actions.push(Action::Compose);
    }
    ui.add_space(10.0);
    if secondary(ui, Icon::Upload, "Import a track", !busy) {
      if let Some(file) = pick_files(&[RefKind::Audio], false).into_iter().next() {
        actions.push(Action::Import(file));
      }
    }
    if has_track {
      field(ui, "Vocal stem (optional)");
      ui.label(small(if page.project.vocals.is_some() { "A vocal stem is in place. Lip sync follows it instead of the full mix." } else { "If the app that made your song exports stems, bring the vocal one in: lip sync follows a clean vocal better. Without it, the vocal is separated for you on fal when lip sync first runs." }));
      ui.add_space(6.0);
      if secondary(ui, Icon::Mic, if page.project.vocals.is_some() { "Replace the vocal stem" } else { "Import a vocal stem" }, !busy) {
        if let Some(file) = pick_files(&[RefKind::Audio], false).into_iter().next() {
          actions.push(Action::ImportVocals(file));
        }
      }
    }
    next_button(ui, Stage::Analysis, has_track, actions);
  });
}

// --- Music Analysis --------------------------------------------------------------------------

fn analysis(page: &mut MusicVideoPage, ui: &mut Ui, env: &mut Env<'_>, stage: Rect, panel_rect: Rect, actions: &mut Vec<Action>) {
  let busy = page.busy();
  column(ui, stage, "analysis", |ui| {
    heading(ui, Stage::Analysis, "Your track", "Where the video should breathe, build and hit.");
    let (Some(track), Some(grid)) = (&page.project.track, &page.project.grid) else {
      empty(ui, Icon::Drum, "Not analysed yet", "Analysing finds the beat on your Mac and has Gemini listen for the sections.");
      return;
    };
    let read = &page.project.read;
    card(ui, "Creative read".to_owned(), |ui| {
      if !read.mood.is_empty() {
        ui.label(widgets::heading(&read.mood, 24.0));
        ui.add_space(4.0);
      }
      ui.horizontal_wrapped(|ui| {
        chip(ui, &format!("{:.0} BPM", grid.bpm));
        chip(ui, &format!("{} sections", page.project.sections.len()));
        chip(ui, &format!("{} beats", grid.beats.len()));
        chip(ui, &format_time(grid.duration as f32));
        if !read.genre.is_empty() {
          chip(ui, &read.genre);
        }
      });
      if !read.summary.is_empty() {
        ui.add_space(8.0);
        ui.label(body(&read.summary));
      }
      ui.add_space(10.0);
      waveform::player(ui, ui.id().with("mv-analysis"), env.audio, &file_url(&track.path), Some(grid.duration as f32), false);
    });
    card(ui, "Song structure".to_owned(), |ui| {
      let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), TIMELINE_HEIGHT + 18.0), Sense::hover());
      let lane = Rect::from_min_size(bar.min + vec2(0.0, 18.0), vec2(bar.width(), TIMELINE_HEIGHT));
      let x = |t: f64| lane.left() + lane.width() * (t / grid.duration.max(0.001)).clamp(0.0, 1.0) as f32;
      for section in &page.project.sections {
        let block = Rect::from_min_max(pos2(x(section.start) + 1.0, lane.top()), pos2(x(section.end) - 1.0, lane.bottom()));
        ui.painter().rect_filled(block, theme::RADIUS, energy_colour(section.energy));
        if block.width() > 44.0 {
          ui.painter().with_clip_rect(block).text(block.left_center() + vec2(6.0, 0.0), Align2::LEFT_CENTER, &section.name, FontId::new(11.5, theme::semibold()), theme::INVERT_FG);
        }
        ui.painter().text(pos2(block.left(), bar.top()), Align2::LEFT_TOP, format_time(section.start as f32), FontId::new(10.0, theme::mono()), theme::FAINT);
      }
      // Bar lines under the sections.
      for beat in grid.beats.iter().skip(grid.downbeat % 4).step_by(4) {
        ui.painter().vline(x(*beat), lane.bottom()..=lane.bottom() + 4.0, egui::Stroke::new(1.0, theme::LINE));
      }
      ui.add_space(8.0);
      ui.label(small("Brighter blocks are higher energy and cut faster. The ticks are bar lines; every cut lands on a beat."));
    });
    if !page.project.lines.is_empty() {
      card(ui, "Lyrics, as sung".to_owned(), |ui| {
        for line in &page.project.lines {
          ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format_time(line.time as f32)).size(12.0).family(theme::mono()).color(theme::FAINT));
            ui.label(body(&line.text));
          });
        }
      });
    }
    if let Some(note) = &page.analysis_note {
      ui.label(small(note));
    }
  });
  let line = page.progress_line().filter(|_| page.running_here());
  let analysed = page.project.grid.is_some();
  panel(ui, panel_rect, "Music Analysis", "The beat is found on your Mac, free. Gemini listens to the track for its mood, sections and timed lyrics.", |ui| {
    error_line(ui, page.error.as_deref());
    if run_button(ui, Icon::Drum, if analysed { "Analyse again" } else { "Analyse the track" }, Some(A_CENT.to_owned()), page.project.track.is_some() && !busy, line.as_deref()) {
      actions.push(Action::Analyse);
    }
    next_button(ui, Stage::Style, analysed, actions);
  });
}

// --- Visual Style ----------------------------------------------------------------------------

fn style(page: &mut MusicVideoPage, ui: &mut Ui, stage: Rect, panel_rect: Rect, actions: &mut Vec<Action>) {
  let busy = page.busy();
  // While the writer is redoing this stage its reply would replace anything typed.
  let running = page.running_here();
  let mut changed = false;
  column(ui, stage, "style", |ui| {
    heading(ui, Stage::Style, "Visual style", "The look guiding every picture in the film. Click any part to edit it.");
    let Some(visual) = &mut page.project.visual else {
      empty(ui, Icon::Wand, "No style yet", "Say how you want it to look in the panel, or leave that blank and let the writer decide from the track.");
      return;
    };
    if running {
      ui.disable();
    }
    changed |= edit_heading(ui, "mv-style-statement", &mut visual.statement, "The look in one sentence", 20.0);
    if ui.add(egui::Button::new(egui::RichText::new("Work on this with AI").size(12.0).color(theme::MUTED)).frame(false)).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
      actions.push(Action::Workshop(Target::Statement));
    }
    ui.add_space(10.0);
    card(ui, "Colour palette".to_owned(), |ui| {
      let mut removed = None;
      for (i, swatch) in visual.palette.iter_mut().enumerate() {
        ui.horizontal(|ui| {
          let (dot, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
          ui.painter().rect(dot, theme::RADIUS, colour(&swatch.hex), theme::hairline(), egui::StrokeKind::Inside);
          changed |= edit_line(ui, &format!("mv-swatch-name-{i}"), &mut swatch.name, "Colour name", body_font(), 220.0);
          changed |= edit_line(ui, &format!("mv-swatch-hex-{i}"), &mut swatch.hex, "#rrggbb", FontId::new(12.5, theme::mono()), 80.0);
          if ui.add(egui::Button::new(small("Remove")).frame(false)).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            removed = Some(i);
          }
        });
      }
      if let Some(i) = removed {
        visual.palette.remove(i);
        changed = true;
      }
      if ui.add(egui::Button::new(small("+ Add a colour")).frame(false)).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        visual.palette.push(Swatch { name: String::new(), hex: "#808080".to_owned() });
        changed = true;
      }
    });
    changed |= edit_card(ui, "Lighting", &mut visual.lighting, 2, Target::Lighting, actions);
    changed |= edit_card(ui, "Texture", &mut visual.texture, 2, Target::Texture, actions);
    let asked = workable_card(ui, "Common style block \u{b7} added to every image and clip".to_owned(), |ui| {
      changed |= ui.add(egui::TextEdit::multiline(&mut visual.block).desired_rows(3).desired_width(f32::INFINITY).frame(egui::Frame::NONE)).changed();
    });
    if asked {
      actions.push(Action::Workshop(Target::StyleBlock));
    }
  });
  let line = page.progress_line().filter(|_| page.running_here());
  let styled = page.project.visual.is_some();
  panel(ui, panel_rect, "Visual Style", "Everything here can be edited. The common style block is what steers every picture; pictures already made keep the look they had.", |ui| {
    field(ui, "Your direction (optional)");
    let before = page.project.direction.clone();
    prompt_field(ui, "mv-direction", &mut page.project.direction, "Gritty black-and-white 1990s rave video in a derelict rail tunnel, harsh single light, heavy grain\u{2026}");
    changed |= before != page.project.direction;
    field(ui, "Frame");
    if let Some(i) = segmented(ui, &["16:9 wide", "9:16 tall"], usize::from(page.project.aspect == Aspect::Tall)) {
      page.project.aspect = if i == 0 { Aspect::Wide } else { Aspect::Tall };
      changed = true;
    }
    writer_choice(ui, &mut page.writer);
    error_line(ui, page.error.as_deref());
    if run_button(ui, Icon::Wand, if styled { "Design it again" } else { "Design the style" }, Some(CENTS.to_owned()), !busy, line.as_deref()) {
      actions.push(Action::Style);
    }
    next_button(ui, Stage::Brief, styled, actions);
  });
  if changed {
    page.touch(ui);
  }
}

// --- Creative Brief --------------------------------------------------------------------------

fn brief(page: &mut MusicVideoPage, ui: &mut Ui, stage: Rect, panel_rect: Rect, actions: &mut Vec<Action>) {
  let busy = page.busy();
  // While the writer is redoing this stage its reply would replace anything typed.
  let running = page.running_here();
  let mut changed = false;
  column(ui, stage, "brief", |ui| {
    heading(ui, Stage::Brief, "Creative brief", "The idea connecting your track, the performer and the visual world. Click any part to edit it.");
    let Some(brief) = &mut page.project.brief else {
      empty(ui, Icon::Pencil, "No brief yet", "The writer invents the film's idea, an original performer, a venue and a few signature props.");
      return;
    };
    if running {
      ui.disable();
    }
    changed |= edit_heading(ui, "mv-brief-title", &mut brief.title, "The film's title", 22.0);
    ui.add_space(10.0);
    changed |= edit_card(ui, "Logline", &mut brief.logline, 2, Target::Logline, actions);
    changed |= edit_card(ui, "Core drive", &mut brief.drive, 2, Target::Drive, actions);
    changed |= edit_card(ui, "Director's vision", &mut brief.vision, 3, Target::Vision, actions);
    changed |= edit_card(ui, "Editing strategy", &mut brief.editing, 2, Target::Editing, actions);
    let asked = workable_card(ui, "Visual motifs \u{b7} one per line".to_owned(), |ui| {
      let mut motifs = brief.motifs.join("\n");
      if edit_text(ui, "mv-brief-motifs", &mut motifs, 3) {
        brief.motifs = motifs.split('\n').map(str::to_owned).collect();
        changed = true;
      }
    });
    if asked {
      actions.push(Action::Workshop(Target::Motifs));
    }
    changed |= subject_card(ui, "Character", "character", &mut brief.character, true, Target::Character, actions);
    changed |= subject_card(ui, "Venue", "venue", &mut brief.venue, true, Target::Venue, actions);
    let mut removed = None;
    for (i, prop) in brief.props.iter_mut().enumerate() {
      changed |= subject_card(ui, "Prop", &format!("prop-{i}"), prop, false, Target::Prop(i), actions);
      if prop.name.trim().is_empty() && prop.description.trim().is_empty() && secondary(ui, Icon::Plus, "Remove this empty prop", true) {
        removed = Some(i);
      }
    }
    if let Some(i) = removed {
      brief.props.remove(i);
      changed = true;
    }
    if secondary(ui, Icon::Plus, "Add a prop", true) {
      brief.props.push(Subject::default());
      changed = true;
    }
  });
  if changed {
    // The reference images take the new names and wording, and keep their pictures.
    page.project.follow_edited_brief();
    page.touch(ui);
  }
  let line = page.progress_line().filter(|_| page.running_here());
  let briefed = page.project.brief.is_some();
  panel(ui, panel_rect, "Creative Brief", "Everything here can be edited. The performer, venue and props become the reference images; scenes already written keep their wording until you plan them again.", |ui| {
    writer_choice(ui, &mut page.writer);
    error_line(ui, page.error.as_deref());
    if run_button(ui, Icon::Pencil, if briefed { "Write it again" } else { "Write the brief" }, Some(CENTS.to_owned()), page.project.visual.is_some() && !busy, line.as_deref()) {
      actions.push(Action::Brief);
    }
    next_button(ui, Stage::References, briefed, actions);
  });
}

/// The performer, the venue or a prop, edited in place; returns whether anything changed.
fn subject_card(ui: &mut Ui, title: &str, id: &str, subject: &mut Subject, has_role: bool, target: Target, actions: &mut Vec<Action>) -> bool {
  let mut changed = false;
  let asked = workable_card(ui, title.to_owned(), |ui| {
    changed |= edit_line(ui, &format!("mv-brief-{id}-name"), &mut subject.name, "Name", FontId::new(14.0, theme::semibold()), f32::INFINITY);
    if has_role {
      changed |= edit_line(ui, &format!("mv-brief-{id}-role"), &mut subject.role, if title == "Venue" { "Its mood" } else { "Their role" }, FontId::new(12.5, egui::FontFamily::Proportional), f32::INFINITY);
    }
    ui.add_space(2.0);
    changed |= edit_text(ui, &format!("mv-brief-{id}-description"), &mut subject.description, 3);
  });
  if asked {
    actions.push(Action::Workshop(target));
  }
  changed
}

// --- Reference Images ------------------------------------------------------------------------

fn references(page: &mut MusicVideoPage, ui: &mut Ui, env: &mut Env<'_>, stage: Rect, panel_rect: Rect, actions: &mut Vec<Action>) {
  let busy = page.busy();
  let mut changed = false;
  let mut redraw = None;
  let mut upload = None;
  let mut download = None;
  page.reference_changes.resize(page.project.references.len(), String::new());
  column_wide(ui, stage, "references", |ui| {
    heading(ui, Stage::References, "Reference images", "The performer, the location and the signature details. Every scene is drawn from these, so they stay the same from shot to shot.");
    if page.project.references.is_empty() {
      empty(ui, Icon::Image, "Nothing to draw yet", "Write the creative brief first.");
      return;
    }
    let (columns, width) = grid_columns(ui.available_width(), REFERENCE_MIN_WIDTH);
    let height = width * 0.62;
    for row in 0..page.project.references.len().div_ceil(columns) {
      ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = CARD_GAP;
        for index in row * columns..((row + 1) * columns).min(page.project.references.len()) {
          let reference = &mut page.project.references[index];
          let change = &mut page.reference_changes[index];
          ui.vertical(|ui| {
            ui.set_width(width);
            let (rect, picture) = ui.allocate_exact_size(vec2(width, height), Sense::click());
            thumbnail(ui, rect, env.cache, reference.image.as_deref(), Icon::Image);
            if let Some(image) = reference.image.as_ref().filter(|_| picture.on_hover_text("Double-click to see it full size").double_clicked()) {
              env.requests.push(AppRequest::Preview(file_url(image)));
            }
            if let Some(error) = &reference.error {
              failed_badge(ui, rect, error);
            }
            ui.add_space(6.0);
            // The name has a line to itself, cut short if need be: beside the buttons a long one ran under them.
            ui.add(egui::Label::new(egui::RichText::new(&reference.name).size(14.0).family(theme::semibold()).color(theme::INK)).truncate()).on_hover_text(&reference.name);
            ui.horizontal(|ui| {
              ui.label(egui::RichText::new(kind_title(reference.kind)).size(11.0).family(theme::mono()).color(theme::FAINT));
              ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                  if widgets::ghost(ui, Icon::RotateCw, None, "Draw this one again from its description", false).clicked() {
                    redraw = Some(index);
                  }
                  if widgets::ghost(ui, Icon::Upload, None, "Upload a picture: your own, or this one after editing it elsewhere", false).clicked() {
                    upload = Some(index);
                  }
                  if reference.image.is_some() && widgets::ghost(ui, Icon::Download, None, "Save a copy of this picture to edit in another app", false).clicked() {
                    download = Some(index);
                  }
                });
              });
            });
            if ui.add_enabled(!busy, egui::Button::new(egui::RichText::new("Work on this with AI").size(12.0).color(theme::MUTED)).frame(false)).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Talk this description through with the writer, then put the result back here").clicked() {
              actions.push(Action::Workshop(Target::Reference(index)));
            }
            changed |= ui.add(egui::TextEdit::multiline(&mut reference.description).id_salt(("mv-reference", index)).desired_rows(3).desired_width(width).font(FontId::new(12.5, egui::FontFamily::Proportional))).changed();
            // Changing the picture itself, rather than drawing a new one from the description.
            if reference.image.is_some() {
              ui.add_space(4.0);
              ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(change).id_salt(("mv-reference-change", index)).desired_width(width - 44.0).font(FontId::new(12.5, egui::FontFamily::Proportional)).hint_text(egui::RichText::new("Change it: a longer coat\u{2026}").color(theme::FAINT)));
                ui.add_enabled_ui(!busy && !change.trim().is_empty(), |ui| {
                  if widgets::ghost(ui, Icon::Wand, None, "Change the picture as written, keeping the rest", false).clicked() {
                    actions.push(Action::ChangeReference(index, std::mem::take(change)));
                  }
                });
              });
            }
          });
        }
      });
      ui.add_space(CARD_GAP);
    }
  });
  if changed {
    // The brief follows its references' wording: scenes are drawn from both.
    page.project.follow_edited_references();
    page.touch(ui);
  }
  if let Some(index) = redraw {
    actions.push(Action::References(vec![index]));
  }
  if let Some(index) = download {
    let reference = &page.project.references[index];
    let extension = reference.image.as_ref().and_then(|p| p.extension()).and_then(|e| e.to_str()).unwrap_or("png");
    let name = format!("{}.{extension}", reference.name.trim().replace(['/', '\\', ':'], "-"));
    if let Some(to) = rfd::FileDialog::new().set_file_name(&name).save_file() {
      actions.push(Action::DownloadReference(index, to));
    }
  }
  if let Some(file) = upload.and_then(|_| pick_files(&[RefKind::Image], false).into_iter().next()) {
    actions.push(Action::UploadReference(upload.unwrap_or_default(), file));
  }
  let missing: Vec<usize> = page.project.references.iter().enumerate().filter(|(_, r)| r.image.is_none()).map(|(i, _)| i).collect();
  let all: Vec<usize> = (0..page.project.references.len()).collect();
  let targets = if missing.is_empty() { all } else { missing };
  let cost = env.backend.estimate_song(&SongEstimate::Stills { count: targets.len(), pro: page.pro(), large: false });
  let line = page.progress_line().filter(|_| page.running_here());
  let any = page.project.references.iter().any(|r| r.image.is_some());
  let pro = page.pro();
  panel(ui, panel_rect, "Reference Images", "For each one: draw it from its description, work on the description with AI, write a change to the picture, or save the picture, edit it in another app and upload it back to the same card. Names, and adding or removing props, are on the Creative Brief.", |ui| {
    picture_model(ui, pro);
    error_line(ui, page.error.as_deref());
    let label = if any && targets.len() == page.project.references.len() { format!("Draw all {} again", targets.len()) } else { format!("Draw {} image{}", targets.len(), plural(targets.len())) };
    if run_button(ui, Icon::Image, &label, cost.map(estimate), !targets.is_empty() && !busy, line.as_deref()) {
      actions.push(Action::References(targets.clone()));
    }
    stop_button(ui, page.running_here(), actions);
    ui.label(small("Scenes can be drawn without reference images, from the descriptions alone, but the performer will drift from shot to shot."));
    next_button(ui, Stage::Scenes, page.project.brief.is_some(), actions);
  });
}

// --- Scenes ----------------------------------------------------------------------------------

fn scenes(page: &mut MusicVideoPage, ui: &mut Ui, env: &mut Env<'_>, stage: Rect, panel_rect: Rect, actions: &mut Vec<Action>) {
  let busy = page.busy();
  let quality = page.quality();
  column_wide(ui, stage, "scenes", |ui| {
    heading(ui, Stage::Scenes, "Scene planning", "The film cut into scenes on the beat, each with its opening frame. Click one to edit it, double-click to see it full size, and Cmd-click or Shift-click to select several and redraw them together.");
    if page.project.shots.is_empty() {
      empty(ui, Icon::Film, "No scenes yet", "Planning cuts the song into scenes on its beats and has the writer describe each one.");
      return;
    }
    scene_grid(page, ui, env.cache, env.requests, false);
  });
  let slots = page.project.grid.as_ref().map_or(0, |grid| plan::slots(grid, &page.project.sections, page.project.pace).len());
  // The final pass redraws the stills the draft settled on; the draft draws whatever is missing.
  let drawable = |shot: &&crate::backend::local::song::project::Shot| !shot.image.trim().is_empty() && (quality == Quality::Draft || shot.draft.still.is_some());
  let missing: Vec<usize> = page.project.shots.iter().enumerate().filter(|(_, s)| drawable(s) && s.take(quality).still.is_none()).map(|(i, _)| i).collect();
  let all: Vec<usize> = page.project.shots.iter().enumerate().filter(|(_, s)| drawable(s)).map(|(i, _)| i).collect();
  let only_missing = !missing.is_empty();
  let targets = if only_missing { missing } else { all };
  let (pro, large) = (page.pro(), quality == Quality::Final);
  let cost = env.backend.estimate_song(&SongEstimate::Stills { count: targets.len(), pro, large });
  // The scenes picked together in the grid, less any that can't be drawn in this pass.
  page.picked.retain(|i| *i < page.project.shots.len());
  let chosen: Vec<usize> = page.picked.iter().copied().filter(|i| drawable(&&page.project.shots[*i])).collect();
  // In the draft pass, picked stills can be redrawn with the better model the final pass uses.
  let can_better = quality == Quality::Draft;
  let better = can_better && page.better_redraw;
  let chosen_cost = env.backend.estimate_song(&SongEstimate::Stills { count: chosen.len(), pro: pro || better, large });
  let one = env.backend.estimate_song(&SongEstimate::Stills { count: 1, pro: pro || better, large });
  let mut clear_picked = false;
  let line = page.progress_line().filter(|_| page.running_here());
  let planned = !page.project.shots.is_empty();
  let drawn = page.project.shots.iter().any(|s| s.has_still());
  let sung = page.project.shots.iter().filter(|s| s.sing).count();
  let fal = env.backend.fal_connected();
  let mut changed = false;
  panel(ui, panel_rect, "Scenes", "Planning again replaces the scenes and their stills, in both passes.", |ui| {
    field(ui, "Pace");
    if let Some(i) = segmented(ui, &Pace::ALL.map(Pace::label), Pace::ALL.iter().position(|p| *p == page.project.pace).unwrap_or(1)) {
      page.project.pace = Pace::ALL[i];
      changed = true;
    }
    ui.label(small(&format!("{slots} scenes at this pace. Faster pacing means more scenes, so more stills and clips to pay for.")));
    field(ui, "Lip sync");
    changed |= ui.checkbox(&mut page.project.lip_sync, "The performer sings on camera").changed();
    ui.label(small(&lip_sync_note(page.project.lip_sync, fal, planned, sung)));
    writer_choice(ui, &mut page.writer);
    error_line(ui, page.error.as_deref());
    if run_button(ui, Icon::List, if planned { "Plan again" } else { "Plan the scenes" }, Some(A_CENT.to_owned()), page.project.brief.is_some() && page.project.grid.is_some() && !busy, None) {
      actions.push(Action::Plan);
    }
    if planned {
      field(ui, &format!("Stills \u{b7} {} pass", quality.label().to_lowercase()));
      picture_model(ui, pro);
      let label = match (quality, only_missing) {
        (Quality::Draft, true) => format!("Draw {} still{}", targets.len(), plural(targets.len())),
        (Quality::Draft, false) => format!("Draw all {} again", targets.len()),
        (Quality::Final, true) => format!("Redraw {} still{}", targets.len(), plural(targets.len())),
        (Quality::Final, false) => format!("Redraw all {} again", targets.len()),
      };
      if run_button(ui, Icon::Image, &label, cost.map(estimate), !targets.is_empty() && !busy, line.as_deref()) {
        actions.push(Action::Stills(targets.clone(), false));
      }
      if quality == Quality::Final && targets.is_empty() {
        ui.label(small("The final pass redraws draft stills. Draw them in the Draft pass first."));
      }
      stop_button(ui, page.running_here(), actions);
    }
    if can_better && (!page.picked.is_empty() || page.selected.is_some()) {
      field(ui, "Redraw with");
      ui.checkbox(&mut page.better_redraw, "The better model (Nano Banana Pro)").on_hover_text("Holds the performer's likeness better than the draft's faster model, at about twice the price per still. Applies to the redraw buttons below, not to drawing the whole storyboard.");
    }
    if !page.picked.is_empty() {
      field(ui, &format!("{} scene{} selected", page.picked.len(), plural(page.picked.len())));
      if run_button(ui, Icon::RotateCw, &format!("Redraw {} selected", chosen.len()), chosen_cost.map(estimate), !busy && !chosen.is_empty(), None) {
        actions.push(Action::Stills(chosen.clone(), better));
      }
      if chosen.len() < page.picked.len() {
        ui.label(small("Some of the selected scenes have no draft still or no description yet, so they are left out."));
      }
      if secondary(ui, Icon::X, "Clear the selection", true) {
        clear_picked = true;
      }
    }
    if let Some(index) = page.selected.filter(|i| *i < page.project.shots.len()) {
      let lip_sync = page.project.lip_sync;
      let shot = &mut page.project.shots[index];
      field(ui, &format!("Scene {} \u{b7} {}", index + 1, shot.slot.section));
      if !shot.slot.lyric.is_empty() {
        ui.label(small(&format!("Sung: {}", shot.slot.lyric)));
      }
      ui.label(label("Opening frame"));
      let before = (shot.image.clone(), shot.motion.clone(), shot.sing);
      prompt_field(ui, "mv-scene-image", &mut shot.image, "Who or what is in the frame, the framing, the place and the light");
      ui.add_space(6.0);
      ui.label(label("What moves"));
      prompt_field(ui, "mv-scene-motion", &mut shot.motion, "The performer's movement and the camera's");
      if lip_sync {
        ui.add_space(6.0);
        ui.checkbox(&mut shot.sing, "The performer sings this scene (lip-synced)").on_hover_text("Works best as a close-up with the face towards the camera and the mouth in view");
      }
      changed |= before != (shot.image.clone(), shot.motion.clone(), shot.sing);
      if let Some(error) = &shot.error {
        error_line(ui, Some(error));
      }
      let can_draw = !shot.image.trim().is_empty() && (quality == Quality::Draft || shot.draft.still.is_some());
      if run_button(ui, Icon::RotateCw, if quality == Quality::Final { "Redraw this still" } else { "Draw this still" }, one.map(estimate), !busy && can_draw, None) {
        actions.push(Action::Stills(vec![index], better));
      }
    }
    next_button(ui, Stage::Shots, drawn, actions);
  });
  if clear_picked {
    page.picked.clear();
  }
  if changed {
    page.touch(ui);
  }
}

/// What the lip-sync setting means right now, and what it still needs.
fn lip_sync_note(on: bool, fal_connected: bool, planned: bool, sung: usize) -> String {
  match (on, fal_connected, planned) {
    (false, _, _) => "Off: the writer keeps to movement and cutaways, with no one mouthing words.".to_owned(),
    (true, false, _) => "Lip sync runs on fal, which isn't connected. Add your fal API key in Settings, then come back.".to_owned(),
    (true, true, false) => "The writer will mark the scenes the performer sings, framed so the sync holds.".to_owned(),
    (true, true, true) if sung == 0 => "No scene is marked as sung yet. Plan again so the writer picks them, or tick scenes yourself.".to_owned(),
    (true, true, true) => format!("{sung} scene{} marked as sung. Their clips are lip-synced on the Shots stage.", plural(sung)),
  }
}

// --- Shots -----------------------------------------------------------------------------------

fn shots(page: &mut MusicVideoPage, ui: &mut Ui, env: &mut Env<'_>, stage: Rect, panel_rect: Rect, actions: &mut Vec<Action>) {
  let busy = page.busy();
  let quality = page.quality();
  let selected_clip = page.selected.and_then(|i| page.project.shots.get(i)).and_then(|s| s.take(quality).best_clip().cloned());
  let mut grid_area = stage;
  if let Some(clip) = &selected_clip {
    // The picked scene's clip plays above the grid.
    let screen = Rect::from_min_max(stage.min, pos2(stage.right(), stage.top() + (stage.height() * 0.45).min(360.0))).shrink(12.0);
    play(page, ui, screen, clip);
    grid_area = Rect::from_min_max(pos2(stage.left(), screen.bottom()), stage.max);
  }
  column_wide(ui, grid_area, "shots", |ui| {
    heading(ui, Stage::Shots, "Video clips", "Veo animates each scene from its still, silent, a little longer than its slot; the final cut trims it to the beat.");
    scene_grid(page, ui, env.cache, env.requests, true);
  });
  let whole = page.whole_clips_of();
  let with_still = page.project.shots.iter().filter(|s| s.take(quality).still.is_some()).count();
  let missing: Vec<usize> = page.project.shots.iter().enumerate().filter(|(_, s)| s.take(quality).still.is_some() && s.take(quality).clip.is_none()).map(|(i, _)| i).collect();
  let all: Vec<usize> = page.project.shots.iter().enumerate().filter(|(_, s)| s.take(quality).still.is_some()).map(|(i, _)| i).collect();
  let only_missing = !missing.is_empty();
  let targets = if only_missing { missing } else { all };
  let (tier, resolution) = (page.tier(), page.resolution().to_owned());
  let seconds = page.project.seconds_to_generate(quality, only_missing, whole);
  let cost = env.backend.estimate_song(&SongEstimate::Clips { seconds, tier, resolution: resolution.clone() });
  let line = page.progress_line().filter(|_| page.running_here());
  let made = page.project.shots.iter().filter(|s| s.take(quality).clip.is_some()).count();
  // Lip sync: the sung scenes that have a clip, and whether the vocal still has to be separated.
  let lip_sync = page.project.lip_sync;
  let fal = env.backend.fal_connected();
  let (unsynced, unsynced_seconds) = page.project.scenes_to_sync(quality, true);
  let (syncable, syncable_seconds) = page.project.scenes_to_sync(quality, false);
  let sync_missing = !unsynced.is_empty();
  let (sync_targets, sync_seconds) = if sync_missing { (unsynced, unsynced_seconds) } else { (syncable, syncable_seconds) };
  let separate_seconds = if page.project.vocals.is_some() { 0.0 } else { page.project.grid.as_ref().map_or(0.0, |g| g.duration) };
  let sync_cost = env.backend.estimate_song(&SongEstimate::LipSync { seconds: sync_seconds, pro: page.pro(), separate_seconds });
  let pass = page.pass();
  panel(ui, panel_rect, "Shots", "The most expensive stage: check the estimate before you run it. Try a few scenes first by picking one and making just its clip.", |ui| {
    field(ui, &format!("Model \u{b7} {} pass", quality.label().to_lowercase()));
    if let Some(i) = segmented(ui, &TIERS.map(|tier| tier.label()), page.tier[pass]) {
      page.tier[pass] = i;
    }
    field(ui, "Resolution");
    if let Some(i) = segmented(ui, &RESOLUTIONS, page.resolution[pass]) {
      page.resolution[pass] = i;
    }
    ui.label(small(if whole.is_some() { "Veo makes 1080p only as 8-second clips, so every scene costs 8 seconds whatever its length." } else { "Clips are 4, 6 or 8 seconds: the shortest that covers the scene." }));
    ui.add_space(4.0);
    ui.label(small(&format!("{made} of {with_still} clips made \u{b7} {seconds} s of video to generate")));
    if with_still == 0 && quality == Quality::Final {
      ui.label(small("Final clips are filmed from final stills. Redraw the stills on the Scenes stage first."));
    }
    error_line(ui, page.error.as_deref());
    let label = if only_missing { format!("Make {} clip{}", targets.len(), plural(targets.len())) } else { format!("Make all {} again", targets.len()) };
    if run_button(ui, Icon::Video, &label, cost.map(estimate), !targets.is_empty() && !busy, line.as_deref()) {
      actions.push(Action::Clips(targets.clone()));
    }
    stop_button(ui, page.running_here(), actions);
    if lip_sync {
      field(ui, "Lip sync");
      let note = match (fal, sync_targets.is_empty()) {
        (false, _) => "Lip sync runs on fal, which isn't connected. Add your fal API key in Settings.".to_owned(),
        (true, true) => "No sung scene has a clip yet. Make the clips first.".to_owned(),
        (true, false) => format!("{} sung scene{}, {sync_seconds:.0} s in all. The mouth is redrawn to the vocal{}; expect to redo a few.", sync_targets.len(), plural(sync_targets.len()), if separate_seconds > 0.0 { ", which is separated from the track first" } else { "" }),
      };
      ui.label(small(&note));
      let label = match (sync_targets.is_empty(), sync_missing) {
        (true, _) => "Lip-sync sung scenes".to_owned(),
        (false, true) => format!("Lip-sync {} scene{}", sync_targets.len(), plural(sync_targets.len())),
        (false, false) => format!("Lip-sync all {} again", sync_targets.len()),
      };
      // Nothing to sync, nothing to price.
      let price = sync_cost.filter(|_| !sync_targets.is_empty()).map(estimate);
      if run_button(ui, Icon::Mic, &label, price, fal && !sync_targets.is_empty() && !busy, None) {
        actions.push(Action::LipSync(sync_targets.clone()));
      }
    }
    if let Some(index) = page.selected.filter(|i| *i < page.project.shots.len()) {
      let shot = &page.project.shots[index];
      let take = shot.take(quality);
      field(ui, &format!("Scene {} \u{b7} {:.1} s", index + 1, shot.slot.seconds()));
      ui.label(body(&shot.motion));
      if shot.sing {
        ui.label(small(if take.synced.is_some() { "Sung and lip-synced." } else { "Sung: not lip-synced yet." }));
      }
      if let Some(error) = &shot.error {
        error_line(ui, Some(error));
      }
      let clip_seconds = u32::from(whole.unwrap_or_else(|| plan::clip_seconds(shot.slot.seconds())));
      let one = env.backend.estimate_song(&SongEstimate::Clips { seconds: clip_seconds, tier, resolution: resolution.clone() });
      if run_button(ui, Icon::RotateCw, if take.clip.is_some() { "Make this clip again" } else { "Make this clip" }, one.map(estimate), !busy && take.still.is_some(), None) {
        actions.push(Action::Clips(vec![index]));
      }
      if lip_sync && shot.sing && take.clip.is_some() {
        let one_sync = env.backend.estimate_song(&SongEstimate::LipSync { seconds: shot.slot.seconds(), pro: quality == Quality::Final, separate_seconds });
        if run_button(ui, Icon::Mic, if take.synced.is_some() { "Lip-sync this scene again" } else { "Lip-sync this scene" }, one_sync.map(estimate), fal && !busy, None) {
          actions.push(Action::LipSync(vec![index]));
        }
      }
    }
    next_button(ui, Stage::Final, page.project.shots.iter().any(|s| s.has_clip()), actions);
  });
}

// --- Final Video -----------------------------------------------------------------------------

fn final_video(page: &mut MusicVideoPage, ui: &mut Ui, env: &mut Env<'_>, stage: Rect, panel_rect: Rect, actions: &mut Vec<Action>) {
  let busy = page.busy();
  let quality = page.quality();
  match page.project.cut.clone() {
    Some(cut) => play(page, ui, stage.shrink(12.0), &cut),
    None => column(ui, stage, "final", |ui| {
      heading(ui, Stage::Final, "Final video", "Every clip trimmed to its scene, joined in order and laid under the song.");
      empty(ui, Icon::Film, "Not cut yet", "Cutting runs on your Mac with FFmpeg and costs nothing, so cut as often as you like.");
    }),
  }
  let total = page.project.shots.len();
  let own_clips = page.project.shots.iter().filter(|s| s.take(quality).clip.is_some()).count();
  let any_clip = page.project.shots.iter().filter(|s| s.has_clip()).count();
  let stills_held = page.project.shots.iter().filter(|s| !s.has_clip() && s.has_still()).count();
  let blank = page.project.shots.iter().filter(|s| !s.has_clip() && !s.has_still()).count();
  let unsynced = page.project.scenes_to_sync(quality, true).0.len();
  let line = page.progress_line().filter(|_| page.running_here());
  let mut changed = false;
  let pass = page.pass();
  let _ = env;
  panel(ui, panel_rect, "Final Video", "The film is saved to your library, where it can go on to FilmCraft or Edit Video.", |ui| {
    field(ui, &format!("{} pass", quality.label()));
    let borrowed = any_clip - own_clips;
    let pass_note = match (quality, borrowed) {
      (_, 0) => format!("{own_clips} of {total} scenes have a clip from this pass."),
      (Quality::Final, _) => format!("{own_clips} of {total} scenes have a final clip; {borrowed} will use their draft clip until the final one is made."),
      (Quality::Draft, _) => format!("{own_clips} of {total} scenes have a draft clip; {borrowed} will use their final clip."),
    };
    ui.label(small(&pass_note));
    field(ui, "Finish");
    if let Some(i) = segmented(ui, &Grade::ALL.map(Grade::label), Grade::ALL.iter().position(|g| *g == page.project.grade).unwrap_or(0)) {
      page.project.grade = Grade::ALL[i];
      changed = true;
    }
    ui.label(small("Applied to every shot when the film is cut, so clips from different generations sit together."));
    field(ui, "Resolution");
    if let Some(i) = segmented(ui, &RESOLUTIONS, page.resolution[pass]) {
      page.resolution[pass] = i;
    }
    if blank > 0 {
      ui.add_space(6.0);
      ui.label(egui::RichText::new(format!("{blank} scene{} no still or clip yet. Draw {} on the Scenes stage first.", if blank == 1 { " has" } else { "s have" }, if blank == 1 { "it" } else { "them" })).size(12.0).color(theme::BAD));
    } else if stills_held > 0 {
      ui.add_space(6.0);
      ui.label(small(&format!("{stills_held} scene{} without a clip will hold on {} still.", plural(stills_held), if stills_held == 1 { "its" } else { "their" })));
    }
    if page.project.lip_sync && unsynced > 0 {
      ui.add_space(6.0);
      ui.label(small(&format!("{unsynced} sung scene{} not lip-synced yet, so the mouth won't match there.", if unsynced == 1 { " is" } else { "s are" })));
    }
    error_line(ui, page.error.as_deref());
    if run_button(ui, Icon::Film, if page.project.cut.is_some() { "Cut it again" } else { "Cut the film" }, Some("Free".to_owned()), blank == 0 && page.project.cut_shots(quality).is_some() && !busy, line.as_deref()) {
      actions.push(Action::Cut);
    }
    if let Some(cut) = &page.project.cut {
      ui.add_space(10.0);
      if secondary(ui, Icon::Folder, "Show the file", true) {
        actions.push(Action::Reveal(cut.clone()));
      }
    }
  });
  if changed {
    page.touch(ui);
  }
}

// --- Shared pieces ---------------------------------------------------------------------------

/// The scenes as a grid of cards: the still (or a placeholder), the scene's number, time and
/// section, and its beat. With `clips`, badges say whether the clip has been made and, for sung
/// scenes, whether it's been lip-synced. In the final pass, a scene still on its draft still is
/// tagged so.
fn scene_grid(page: &mut MusicVideoPage, ui: &mut Ui, cache: &mut MediaCache, requests: &mut Vec<AppRequest>, clips: bool) {
  let quality = page.quality();
  let (columns, width) = grid_columns(ui.available_width(), CARD_MIN_WIDTH);
  let picture = match page.project.aspect {
    Aspect::Wide => vec2(width, width * 9.0 / 16.0),
    Aspect::Tall => vec2(width, width * 1.2),
  };
  let mut picked = None;
  for row in 0..page.project.shots.len().div_ceil(columns) {
    ui.horizontal_top(|ui| {
      ui.spacing_mut().item_spacing.x = CARD_GAP;
      for index in row * columns..((row + 1) * columns).min(page.project.shots.len()) {
        let shot = &page.project.shots[index];
        let take = shot.take(quality);
        let (rect, resp) = ui.allocate_exact_size(picture + vec2(0.0, CARD_TEXT_HEIGHT), Sense::click());
        if !ui.is_rect_visible(rect) {
          continue;
        }
        let image = Rect::from_min_size(rect.min, picture);
        thumbnail(ui, image, cache, shot.still_for(quality).map(PathBuf::as_path), Icon::Image);
        let selected = page.selected == Some(index) || (!clips && page.picked.contains(&index));
        if selected || resp.hovered() {
          ui.painter().rect_stroke(image, theme::RADIUS, egui::Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { theme::ACCENT } else { theme::LINE_STRONG }), egui::StrokeKind::Inside);
        }
        if quality == Quality::Final && take.still.is_none() && shot.draft.still.is_some() {
          tag(ui, image, "Draft");
        }
        if let Some(error) = &shot.error {
          failed_badge(ui, image, error);
        } else if clips && take.clip.is_some() {
          badge(ui, image, 0, Icon::Play, theme::ACCENT);
        }
        if !clips && page.picked.contains(&index) {
          badge(ui, image, 2, Icon::Check, theme::ACCENT);
        }
        if shot.sing {
          // Filled once the clip is lip-synced.
          badge(ui, image, 1, Icon::Mic, if take.synced.is_some() { theme::ACCENT } else { Color32::from_black_alpha(170) });
        }
        let text = Rect::from_min_max(pos2(rect.left(), image.bottom() + 6.0), rect.max);
        let caption = format!("{} \u{b7} {} \u{b7} {:.1}s", index + 1, format_time(shot.slot.start as f32), shot.slot.seconds());
        ui.painter().text(text.left_top(), Align2::LEFT_TOP, caption, FontId::new(11.0, theme::mono()), theme::MUTED);
        ui.painter().text(text.right_top(), Align2::RIGHT_TOP, &shot.slot.section, FontId::new(11.0, theme::mono()), theme::FAINT);
        let line = if shot.beat.is_empty() { &shot.image } else { &shot.beat };
        let galley = ui.painter().layout(line.clone(), FontId::new(12.5, egui::FontFamily::Proportional), theme::INK, text.width());
        ui.painter().with_clip_rect(text).galley(text.left_top() + vec2(0.0, 16.0), galley, theme::INK);
        let resp = resp.on_hover_text(&shot.image).on_hover_cursor(egui::CursorIcon::PointingHand);
        // A double click shows the still at full size.
        if let Some(still) = shot.still_for(quality).filter(|_| resp.double_clicked()) {
          requests.push(AppRequest::Preview(file_url(still)));
        }
        if resp.clicked() {
          picked = Some(index);
        }
      }
    });
    ui.add_space(CARD_GAP);
  }
  let Some(index) = picked else {
    return;
  };
  // On the storyboard, Cmd- or Ctrl-click adds or removes a scene from the selection and
  // Shift-click takes every scene from the last one clicked; a plain click starts over.
  let modifiers = ui.input(|i| i.modifiers);
  if !clips && (modifiers.command || modifiers.ctrl || modifiers.shift) {
    if let Some(first) = page.selected.filter(|_| page.picked.is_empty()) {
      page.picked.insert(first);
    }
    if modifiers.shift {
      let from = page.selected.unwrap_or(index);
      page.picked.extend(from.min(index)..=from.max(index));
    } else if !page.picked.remove(&index) {
      page.picked.insert(index);
    }
  } else {
    page.picked.clear();
  }
  if page.selected != Some(index) {
    page.player = None;
  }
  page.selected = Some(index);
}

/// Plays the file at `path` in `rect`, keeping one player for the page.
fn play(page: &mut MusicVideoPage, ui: &mut Ui, rect: Rect, path: &Path) {
  let url = file_url(path);
  if page.player.as_ref().is_none_or(|player| player.url() != url) {
    page.player = Some(VideoPlayer::new(&url));
  }
  if let Some(player) = &mut page.player {
    player.ui(ui, rect, None);
  }
}

/// A picture's small copy fitted into `rect`, or a placeholder icon while there's none.
fn thumbnail(ui: &mut Ui, rect: Rect, cache: &mut MediaCache, picture: Option<&Path>, placeholder: Icon) {
  ui.painter().rect(rect, theme::RADIUS, theme::SUNKEN, theme::hairline(), egui::StrokeKind::Inside);
  let Some(path) = picture else {
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(22.0, 22.0)), placeholder, theme::FAINT);
    return;
  };
  let small = thumb_path(path);
  let url = file_url(if small.exists() { &small } else { path });
  match cache.get(ui.ctx(), &url) {
    Lookup::Ready(texture) => {
      let size = fit_inside(texture.size_vec2(), rect.shrink(1.0).size());
      egui::Image::new(&texture).paint_at(ui, Rect::from_center_size(rect.center(), size));
    },
    Lookup::Loading => icons::paint_spinner(ui, Rect::from_center_size(rect.center(), vec2(20.0, 20.0)), theme::MUTED),
    Lookup::Failed => icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(22.0, 22.0)), Icon::AlertCircle, theme::BAD),
  }
}

/// A round badge at the picture's top-right corner; `slot` 1 sits to the left of slot 0.
fn badge(ui: &Ui, picture: Rect, slot: usize, icon: Icon, fill: Color32) {
  let dot = Rect::from_center_size(picture.right_top() + vec2(-16.0 - 26.0 * slot as f32, 16.0), vec2(22.0, 22.0));
  ui.painter().circle_filled(dot.center(), 11.0, fill);
  icons::paint(ui.painter(), dot.shrink(6.0), icon, Color32::WHITE);
}

/// A small label at the picture's top-left corner.
fn tag(ui: &Ui, picture: Rect, text: &str) {
  let galley = widgets::caps(ui, text, 9.5, theme::mono_bold(), 1.0, Color32::WHITE);
  let chip = Rect::from_min_size(picture.left_top() + vec2(8.0, 8.0), galley.size() + vec2(10.0, 6.0));
  ui.painter().rect_filled(chip, theme::RADIUS, Color32::from_black_alpha(170));
  ui.painter().galley(chip.min + vec2(5.0, 3.0), galley, Color32::WHITE);
}

fn failed_badge(ui: &mut Ui, picture: Rect, error: &str) {
  badge(ui, picture, 0, Icon::AlertCircle, theme::DANGER);
  let dot = Rect::from_center_size(picture.right_top() + vec2(-16.0, 16.0), vec2(22.0, 22.0));
  ui.interact(dot, ui.id().with(("mv-failed", picture.min.x as i32, picture.min.y as i32)), Sense::hover()).on_hover_text(error);
}

/// How many cards fit across `available`, and how wide each is.
fn grid_columns(available: f32, min_width: f32) -> (usize, f32) {
  let columns = (((available + CARD_GAP) / (min_width + CARD_GAP)).floor() as usize).max(1);
  (columns, (available - CARD_GAP * (columns - 1) as f32) / columns as f32)
}

/// The middle column, scrolling, at a comfortable reading width.
fn column(ui: &mut Ui, rect: Rect, id: &str, body_ui: impl FnOnce(&mut Ui)) {
  scroll(ui, rect, id, TEXT_COLUMN, body_ui);
}

/// The middle column at full width, for grids.
fn column_wide(ui: &mut Ui, rect: Rect, id: &str, body_ui: impl FnOnce(&mut Ui)) {
  scroll(ui, rect, id, f32::INFINITY, body_ui);
}

fn scroll(ui: &mut Ui, rect: Rect, id: &str, max_width: f32, body_ui: impl FnOnce(&mut Ui)) {
  let inner = rect.shrink2(vec2(24.0, 20.0));
  let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
  egui::ScrollArea::vertical().id_salt(("mv-stage", id)).auto_shrink([false, false]).show(&mut child, |ui| {
    ui.set_width((inner.width() - 12.0).min(max_width));
    body_ui(ui);
    ui.add_space(12.0);
  });
}

/// A stage's number, title and one-line explanation.
fn heading(ui: &mut Ui, stage: Stage, title: &str, blurb: &str) {
  widgets::hud(ui, &format!("{:02} / {}", stage.number(), stage.label()), theme::FAINT);
  ui.add_space(6.0);
  ui.label(widgets::heading(title, 30.0));
  ui.add_space(4.0);
  ui.label(egui::RichText::new(blurb).size(13.5).color(theme::MUTED));
  ui.add_space(16.0);
}

/// A bordered block with a small label over its content.
/// A card with a link in its header to work on its contents with the writer; returns whether the
/// link was clicked.
fn workable_card(ui: &mut Ui, title: String, content: impl FnOnce(&mut Ui)) -> bool {
  let mut asked = false;
  egui::Frame::new().fill(theme::WASH).stroke(theme::hairline()).corner_radius(theme::RADIUS).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
    ui.set_width(ui.available_width());
    ui.horizontal(|ui| {
      widgets::hud(ui, &title, theme::FAINT);
      ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        asked = ui.add(egui::Button::new(egui::RichText::new("Work on this with AI").size(12.0).color(theme::MUTED)).frame(false)).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Talk this section through with the writer, then put the result back here").clicked();
      });
    });
    ui.add_space(6.0);
    content(ui);
  });
  ui.add_space(CARD_GAP);
  asked
}

fn card(ui: &mut Ui, title: String, content: impl FnOnce(&mut Ui)) {
  egui::Frame::new().fill(theme::WASH).stroke(theme::hairline()).corner_radius(theme::RADIUS).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
    ui.set_width(ui.available_width());
    widgets::hud(ui, &title, theme::FAINT);
    ui.add_space(6.0);
    content(ui);
  });
  ui.add_space(CARD_GAP);
}

/// A card whose text is edited in place or worked on with the writer; returns whether it changed.
fn edit_card(ui: &mut Ui, title: &str, text: &mut String, rows: usize, target: Target, actions: &mut Vec<Action>) -> bool {
  let mut changed = false;
  if workable_card(ui, title.to_owned(), |ui| changed = edit_text(ui, title, text, rows)) {
    actions.push(Action::Workshop(target));
  }
  changed
}

/// Body text edited in place, with no box of its own; returns whether it changed.
fn edit_text(ui: &mut Ui, id: &str, text: &mut String, rows: usize) -> bool {
  ui.add(egui::TextEdit::multiline(text).id_salt(id).frame(egui::Frame::NONE).desired_rows(rows).desired_width(f32::INFINITY).font(body_font()).text_color(theme::INK).hint_text(egui::RichText::new("Click to write\u{2026}").color(theme::FAINT))).changed()
}

/// One line edited in place in `font`; returns whether it changed.
fn edit_line(ui: &mut Ui, id: &str, text: &mut String, hint: &str, font: FontId, width: f32) -> bool {
  ui.add(egui::TextEdit::singleline(text).id_salt(id).frame(egui::Frame::NONE).desired_width(width).font(font).text_color(theme::INK).hint_text(egui::RichText::new(hint).color(theme::FAINT))).changed()
}

/// A heading edited in place, wrapping onto as many lines as it needs.
fn edit_heading(ui: &mut Ui, id: &str, text: &mut String, hint: &str, size: f32) -> bool {
  ui.add(egui::TextEdit::multiline(text).id_salt(id).frame(egui::Frame::NONE).desired_rows(1).desired_width(f32::INFINITY).font(FontId::new(size, theme::display())).text_color(theme::INK).hint_text(egui::RichText::new(hint).color(theme::FAINT))).changed()
}

fn body_font() -> FontId {
  FontId::new(13.5, egui::FontFamily::Proportional)
}

/// The splash a stage shows before it has anything.
fn empty(ui: &mut Ui, icon: Icon, title: &str, text: &str) {
  let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 170.0), Sense::hover());
  ui.painter().rect(rect, theme::RADIUS, theme::fade(theme::INK, 0.015), egui::Stroke::new(1.0, theme::LINE_DASHED), egui::StrokeKind::Inside);
  widgets::corner_marks(ui.painter(), rect);
  icons::paint(ui.painter(), Rect::from_center_size(rect.center() - vec2(0.0, 40.0), vec2(24.0, 24.0)), icon, theme::MUTED);
  ui.painter().text(rect.center() - vec2(0.0, 4.0), Align2::CENTER_CENTER, title, FontId::new(20.0, theme::display()), theme::INK);
  let galley = ui.painter().layout(text.to_owned(), FontId::new(13.0, egui::FontFamily::Proportional), theme::MUTED, (rect.width() - 80.0).min(460.0));
  ui.painter().galley(pos2(rect.center().x - galley.size().x / 2.0, rect.center().y + 16.0), galley, theme::MUTED);
}

/// A multi-line editor in a sunken box; returns whether the text changed.
fn text_box(ui: &mut Ui, id: &str, text: &mut String, hint: &str, rows: usize) -> bool {
  let mut changed = false;
  egui::Frame::new().fill(theme::SUNKEN).stroke(theme::hairline()).corner_radius(theme::RADIUS).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
    changed = ui.add(egui::TextEdit::multiline(text).id_salt(id).frame(egui::Frame::NONE).desired_rows(rows).desired_width(f32::INFINITY).font(FontId::new(14.5, egui::FontFamily::Proportional)).hint_text(egui::RichText::new(hint).color(theme::FAINT))).changed();
  });
  changed
}

fn writer_choice(ui: &mut Ui, writer: &mut Writer) {
  field(ui, "Writer");
  if let Some(i) = segmented(ui, &Writer::ALL.map(Writer::label), Writer::ALL.iter().position(|w| w == writer).unwrap_or(0)) {
    *writer = Writer::ALL[i];
  }
}

/// Which image model the current pass draws with.
fn picture_model(ui: &mut Ui, pro: bool) {
  ui.label(small(if pro { "Final pass: Nano Banana Pro, which holds a face and outfit better and renders finer detail, at about twice the price." } else { "Draft pass: Nano Banana 2, fast and cheap. Switch the quality to Final in the left rail once the direction is settled." }));
}

/// A stage's main button with what it costs beside it, and the run's progress line while it's
/// working. Returns `true` when clicked.
fn run_button(ui: &mut Ui, icon: Icon, text: &str, cost: Option<String>, enabled: bool, progress: Option<&str>) -> bool {
  ui.add_space(12.0);
  let mut clicked = false;
  // Wrapped, so a long label's cost drops to the next line instead of running off the panel.
  ui.horizontal_wrapped(|ui| {
    let working = progress.is_some();
    clicked = ui.add_enabled_ui(enabled && !working, |ui| widgets::button(ui, Some(icon), if working { "Working\u{2026}" } else { text }, widgets::Kind::Accent, 0.0, theme::CONTROL_H)).inner.clicked();
    if working {
      let (spinner, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
      icons::paint_spinner(ui, spinner, theme::MUTED);
    } else if let Some(cost) = cost {
      ui.add_space(6.0);
      ui.label(egui::RichText::new(cost).size(12.5).family(theme::mono()).color(theme::MUTED)).on_hover_text("Estimated at list prices; billed to your own account");
    }
  });
  if let Some(line) = progress {
    ui.add_space(4.0);
    ui.label(small(line));
  }
  clicked
}

fn secondary(ui: &mut Ui, icon: Icon, text: &str, enabled: bool) -> bool {
  ui.add_enabled_ui(enabled, |ui| widgets::button(ui, Some(icon), text, widgets::Kind::Secondary, 0.0, 30.0)).inner.clicked()
}

fn stop_button(ui: &mut Ui, running: bool, actions: &mut Vec<Action>) {
  if running {
    ui.add_space(8.0);
    if widgets::button(ui, Some(Icon::X), "Stop", widgets::Kind::Secondary, 0.0, 30.0).on_hover_text("Stops before the next one starts; the ones in flight finish and are billed").clicked() {
      actions.push(Action::Stop);
    }
  }
}

fn next_button(ui: &mut Ui, next: Stage, enabled: bool, actions: &mut Vec<Action>) {
  ui.add_space(18.0);
  ui.separator();
  ui.add_space(10.0);
  let clicked = ui.add_enabled_ui(enabled, |ui| widgets::button(ui, Some(Icon::ChevronRight), &format!("Next: {}", next.label()), widgets::Kind::Primary, 0.0, theme::CONTROL_H)).inner.clicked();
  if clicked {
    actions.push(Action::Go(next));
  }
}

fn error_line(ui: &mut Ui, error: Option<&str>) {
  if let Some(error) = error {
    ui.add_space(10.0);
    ui.label(egui::RichText::new(error).size(12.0).color(theme::BAD));
  }
}

fn chip(ui: &mut Ui, text: &str) {
  let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(11.5, theme::mono()), theme::MUTED);
  let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(16.0, 8.0), Sense::hover());
  ui.painter().rect(rect, 99.0, theme::WASH, theme::hairline(), egui::StrokeKind::Inside);
  ui.painter().galley(rect.center() - galley.size() / 2.0, galley, theme::MUTED);
}

pub(super) fn label(text: &str) -> egui::RichText {
  egui::RichText::new(text).size(12.0).family(theme::medium()).color(theme::MUTED)
}

pub(super) fn body(text: &str) -> egui::RichText {
  egui::RichText::new(text).size(13.5).color(theme::INK)
}

pub(super) fn small(text: &str) -> egui::RichText {
  egui::RichText::new(text).size(12.0).color(theme::FAINT)
}

fn estimate(cost: f64) -> String {
  format!("\u{2248} {}", usd(cost))
}

fn plural(n: usize) -> &'static str {
  if n == 1 {
    ""
  } else {
    "s"
  }
}

fn energy_colour(energy: Energy) -> Color32 {
  match energy {
    Energy::Low => theme::fade(theme::ACCENT_400, 0.45),
    Energy::Medium => theme::fade(theme::ACCENT_400, 0.75),
    Energy::High => theme::ACCENT_INK,
  }
}

/// A `#rrggbb` swatch as a colour (mid-grey when it isn't one).
fn colour(hex: &str) -> Color32 {
  let digits = hex.trim_start_matches('#');
  match (digits.len(), u32::from_str_radix(digits, 16)) {
    (6, Ok(rgb)) => Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8),
    _ => Color32::GRAY,
  }
}

/// The date of an RFC 3339 timestamp (`2026-11-05`).
fn day(timestamp: &str) -> &str {
  timestamp.get(..10).unwrap_or(timestamp)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn cards_fill_the_width_evenly() {
    let (columns, width) = grid_columns(900.0, 210.0);
    assert_eq!(columns, 4);
    assert!((width * 4.0 + CARD_GAP * 3.0 - 900.0).abs() < 0.01);
    assert_eq!(grid_columns(100.0, 210.0).0, 1);
  }

  #[test]
  fn the_lip_sync_note_says_what_is_still_needed() {
    assert!(lip_sync_note(false, true, true, 3).starts_with("Off"));
    assert!(lip_sync_note(true, false, true, 3).contains("fal API key in Settings"));
    assert!(lip_sync_note(true, true, false, 0).contains("will mark the scenes"));
    assert!(lip_sync_note(true, true, true, 0).contains("No scene is marked"));
    assert_eq!(lip_sync_note(true, true, true, 1), "1 scene marked as sung. Their clips are lip-synced on the Shots stage.");
  }

  #[test]
  fn swatches_and_dates_read_what_theyre_given() {
    assert_eq!(colour("#0a0b0c"), Color32::from_rgb(10, 11, 12));
    assert_eq!(colour("nonsense"), Color32::GRAY);
    assert_eq!(day("2026-11-05T19:04:00+00:00"), "2026-11-05");
    assert_eq!(day("soon"), "soon");
  }
}
