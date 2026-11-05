//! Working on one section of the Visual Style or the Creative Brief with the writer: a
//! conversation about that section alone, whose result goes back into its box only when the user
//! says so.

use egui::{FontId, Key, Ui};

use super::stages::{body, label, small};
use super::{Action, MusicVideoPage, kind_title};
use crate::backend::local::song::project::SongProject;
use crate::backend::local::song::writer::{Subject, Turn};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::widgets;

const WIDTH: f32 = 680.0;
const TRANSCRIPT_HEIGHT: f32 = 190.0;
/// The field name of a section that is one piece of text.
const TEXT: &str = "text";

/// The conversation on screen.
pub(super) struct Workshop {
  pub target: Target,
  /// The section as the conversation has it now: the writer's latest version, or the user's own
  /// edits to it.
  pub fields: Vec<(String, String)>,
  /// The section as it is in the box.
  original: Vec<(String, String)>,
  pub turns: Vec<Turn>,
  pub input: String,
  /// A reply is on its way.
  pub waiting: bool,
  pub error: Option<String>,
}

impl Workshop {
  /// A conversation about `target`, if the project has that section.
  pub fn open(project: &SongProject, target: Target) -> Option<Self> {
    let fields = target.read(project)?;
    Some(Self { target, original: fields.clone(), fields, turns: Vec::new(), input: String::new(), waiting: false, error: None })
  }

  fn changed(&self) -> bool {
    self.fields != self.original
  }
}

/// A section that can be worked on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::pages::tools) enum Target {
  Statement,
  Lighting,
  Texture,
  StyleBlock,
  Logline,
  Drive,
  Vision,
  Editing,
  Motifs,
  Character,
  Venue,
  Prop(usize),
  /// The description a reference image is drawn from, by the reference's index.
  Reference(usize),
}

impl Target {
  /// What the writer is told the section is.
  pub fn title(self) -> &'static str {
    match self {
      Self::Statement => "The look in one sentence",
      Self::Lighting => "Lighting",
      Self::Texture => "Texture",
      Self::StyleBlock => "Common style block (the words added to every image and clip prompt)",
      Self::Logline => "Logline",
      Self::Drive => "Core drive (the arc of the performance)",
      Self::Vision => "Director's vision",
      Self::Editing => "Editing strategy",
      Self::Motifs => "Visual motifs (one per line)",
      Self::Character => "Character (the performer)",
      Self::Venue => "Venue",
      Self::Prop(_) => "Prop",
      Self::Reference(_) => "Reference image description",
    }
  }

  /// The section as the writer is told it: for a reference image, which picture the words are for.
  pub fn section(self, project: &SongProject) -> String {
    match self {
      Self::Reference(index) => project.references.get(index).map_or_else(|| self.title().to_owned(), |r| format!("The description an image model draws the reference picture of \"{}\" ({}) from", r.name.trim(), kind_title(r.kind).to_lowercase())),
      _ => self.title().to_owned(),
    }
  }

  /// The section's fields as they are in the project.
  fn read(self, project: &SongProject) -> Option<Vec<(String, String)>> {
    let one = |text: &str| Some(vec![(TEXT.to_owned(), text.to_owned())]);
    let subject = |subject: &Subject, role: bool| {
      let mut fields = vec![("name".to_owned(), subject.name.clone())];
      if role {
        fields.push(("role".to_owned(), subject.role.clone()));
      }
      fields.push(("description".to_owned(), subject.description.clone()));
      Some(fields)
    };
    let (style, brief) = (project.visual.as_ref(), project.brief.as_ref());
    match self {
      Self::Statement => one(&style?.statement),
      Self::Lighting => one(&style?.lighting),
      Self::Texture => one(&style?.texture),
      Self::StyleBlock => one(&style?.block),
      Self::Logline => one(&brief?.logline),
      Self::Drive => one(&brief?.drive),
      Self::Vision => one(&brief?.vision),
      Self::Editing => one(&brief?.editing),
      Self::Motifs => one(&brief?.motifs.join("\n")),
      Self::Character => subject(&brief?.character, true),
      Self::Venue => subject(&brief?.venue, true),
      Self::Prop(index) => subject(brief?.props.get(index)?, false),
      Self::Reference(index) => one(&project.references.get(index)?.description),
    }
  }

  /// Puts `fields` into the project's section.
  pub fn write(self, project: &mut SongProject, fields: &[(String, String)]) {
    let field = |key: &str| fields.iter().find(|(k, _)| k == key).map(|(_, text)| text.clone());
    let text = field(TEXT).unwrap_or_default();
    let into = |subject: &mut Subject| {
      subject.name = field("name").unwrap_or_default();
      subject.role = field("role").unwrap_or_default();
      subject.description = field("description").unwrap_or_default();
    };
    if let Self::Reference(index) = self {
      // The brief's performer, venue or prop takes the same wording: scenes are drawn from both.
      if let Some(reference) = project.references.get_mut(index) {
        reference.description = text;
      }
      project.follow_edited_references();
      return;
    }
    let (style, brief) = (project.visual.as_mut(), project.brief.as_mut());
    match self {
      Self::Statement => style.into_iter().for_each(|s| s.statement.clone_from(&text)),
      Self::Lighting => style.into_iter().for_each(|s| s.lighting.clone_from(&text)),
      Self::Texture => style.into_iter().for_each(|s| s.texture.clone_from(&text)),
      Self::StyleBlock => style.into_iter().for_each(|s| s.block.clone_from(&text)),
      Self::Logline => brief.into_iter().for_each(|b| b.logline.clone_from(&text)),
      Self::Drive => brief.into_iter().for_each(|b| b.drive.clone_from(&text)),
      Self::Vision => brief.into_iter().for_each(|b| b.vision.clone_from(&text)),
      Self::Editing => brief.into_iter().for_each(|b| b.editing.clone_from(&text)),
      Self::Motifs => brief.into_iter().for_each(|b| b.motifs = text.lines().map(|m| m.trim().to_owned()).filter(|m| !m.is_empty()).collect()),
      Self::Character => brief.into_iter().for_each(|b| into(&mut b.character)),
      Self::Venue => brief.into_iter().for_each(|b| into(&mut b.venue)),
      Self::Prop(index) => brief.and_then(|b| b.props.get_mut(index)).into_iter().for_each(into),
      Self::Reference(_) => {},
    }
  }
}

/// The conversation's window, when one is open.
pub(super) fn show(page: &mut MusicVideoPage, ui: &mut Ui, actions: &mut Vec<Action>) {
  let busy = page.busy();
  let Some(workshop) = &mut page.workshop else {
    return;
  };
  let waiting = workshop.waiting;
  let dismissed = widgets::modal(ui.ctx(), egui::Id::new("mv-workshop"), WIDTH, |ui| {
    widgets::hud(ui, "Work on this section", theme::FAINT);
    ui.add_space(4.0);
    ui.label(widgets::heading(workshop.target.title(), 20.0));
    ui.add_space(4.0);
    ui.label(small("Say what to change. The writer touches this section only, and nothing changes in your video until you put it back."));
    ui.add_space(12.0);

    // The section as it stands; the user can type into it too.
    let subject = workshop.fields.len() > 1;
    for (key, text) in &mut workshop.fields {
      if subject {
        ui.label(label(field_label(key)));
        ui.add_space(2.0);
      }
      let rows = if key == "name" || key == "role" { 1 } else { 4 };
      egui::Frame::new().fill(theme::SUNKEN).stroke(theme::hairline()).corner_radius(theme::RADIUS).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.add_enabled(!waiting, egui::TextEdit::multiline(text).id_salt(("mv-workshop-field", key.as_str())).frame(egui::Frame::NONE).desired_rows(rows).desired_width(f32::INFINITY).font(FontId::new(13.5, egui::FontFamily::Proportional)).text_color(theme::INK));
      });
      ui.add_space(8.0);
    }
    ui.label(small(if workshop.changed() { "This differs from what's in the box." } else { "This is what's in the box now." }));
    ui.add_space(10.0);

    if !workshop.turns.is_empty() || waiting {
      egui::ScrollArea::vertical().id_salt("mv-workshop-turns").max_height(TRANSCRIPT_HEIGHT).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
        for turn in &workshop.turns {
          ui.label(label(if turn.from_user { "You" } else { "Writer" }));
          ui.label(body(&turn.text));
          ui.add_space(8.0);
        }
        if waiting {
          ui.horizontal(|ui| {
            ui.spinner();
            ui.label(small("The writer is working on it\u{2026}"));
          });
        }
      });
      ui.add_space(8.0);
    }
    if let Some(error) = &workshop.error {
      ui.label(egui::RichText::new(error).size(12.5).color(theme::DANGER));
      ui.add_space(6.0);
    }

    let can_send = !busy && !workshop.input.trim().is_empty();
    ui.horizontal(|ui| {
      let input = ui.add_enabled(!waiting, egui::TextEdit::singleline(&mut workshop.input).id_salt("mv-workshop-input").desired_width(ui.available_width() - 110.0).font(FontId::new(13.5, egui::FontFamily::Proportional)).hint_text(egui::RichText::new("Make it colder and emptier, lose the strobe\u{2026}").color(theme::FAINT)));
      let entered = input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
      let clicked = ui.add_enabled_ui(can_send, |ui| widgets::button(ui, Some(Icon::Wand), "Send", widgets::Kind::Accent, 100.0, 30.0).clicked()).inner;
      if (entered || clicked) && can_send {
        actions.push(Action::WorkshopSend);
        input.request_focus();
      }
    });
    if busy && !waiting {
      ui.add_space(4.0);
      ui.label(small("Something else is running; you can send once it has finished."));
    }
    ui.add_space(14.0);
    ui.horizontal(|ui| {
      let put_back = ui.add_enabled_ui(workshop.changed() && !waiting, |ui| widgets::button(ui, Some(Icon::Check), "Put it back in the box", widgets::Kind::Primary, 230.0, 32.0).clicked()).inner;
      if put_back {
        actions.push(Action::WorkshopApply);
      }
      if widgets::button(ui, None, "Close without changing", widgets::Kind::Secondary, 230.0, 32.0).clicked() {
        actions.push(Action::WorkshopClose);
      }
    });
  });
  // A stray click outside shouldn't throw away a version that hasn't been put back.
  if dismissed && !workshop.changed() && !waiting {
    actions.push(Action::WorkshopClose);
  }
}

/// What the writer is told about the rest of the film, so one section stays in keeping with it.
pub(super) fn context(project: &SongProject) -> String {
  let mut lines = vec![format!("The song: {}", project.idea.trim())];
  if let Some(style) = &project.visual {
    lines.push(format!("The look: {} {}", style.statement.trim(), style.block.trim()));
  }
  if let Some(brief) = &project.brief {
    lines.push(format!("The film is \"{}\". {}", brief.title.trim(), brief.logline.trim()));
    lines.push(format!("The arc: {}", brief.drive.trim()));
    lines.push(format!("The performer, {}: {}", brief.character.name.trim(), brief.character.description.trim()));
    lines.push(format!("The venue, {}: {}", brief.venue.name.trim(), brief.venue.description.trim()));
    if !brief.props.is_empty() {
      lines.push(format!("Props: {}", brief.props.iter().map(|p| p.name.trim()).collect::<Vec<_>>().join(", ")));
    }
  }
  lines.join("\n")
}

fn field_label(key: &str) -> &'static str {
  match key {
    "name" => "Name",
    "role" => "Role or mood",
    _ => "Description",
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::backend::local::song::writer::{Brief, Style};

  #[test]
  fn a_section_goes_out_and_comes_back_without_touching_the_rest() {
    let mut project = project();
    let mut workshop = Workshop::open(&project, Target::Venue).unwrap();
    assert_eq!(workshop.fields, [("name".to_owned(), "The Sump Vault".to_owned()), ("role".to_owned(), "oppressive".to_owned()), ("description".to_owned(), "A flooded bunker.".to_owned())]);
    assert!(!workshop.changed());
    workshop.fields[2].1 = "A flooded engine room.".to_owned();
    assert!(workshop.changed());
    let before = project.brief.clone().unwrap();
    workshop.target.write(&mut project, &workshop.fields);
    let after = project.brief.as_ref().unwrap();
    assert_eq!((after.venue.name.as_str(), after.venue.description.as_str()), ("The Sump Vault", "A flooded engine room."));
    assert_eq!((&after.character, &after.props, &after.logline), (&before.character, &before.props, &before.logline));
  }

  #[test]
  fn text_sections_and_motifs_round_trip() {
    let mut project = project();
    Target::Motifs.write(&mut project, &[(TEXT.to_owned(), "rust\n\n black water \n".to_owned())]);
    assert_eq!(project.brief.as_ref().unwrap().motifs, ["rust", "black water"]);
    assert_eq!(Target::Motifs.read(&project).unwrap()[0].1, "rust\nblack water");
    Target::StyleBlock.write(&mut project, &[(TEXT.to_owned(), "cold steel".to_owned())]);
    assert_eq!(project.visual.as_ref().unwrap().block, "cold steel");
    assert!(Workshop::open(&project, Target::Prop(3)).is_none(), "there is no fourth prop");
    assert!(Workshop::open(&SongProject::new(), Target::Logline).is_none(), "nor a brief yet");
  }

  #[test]
  fn a_reference_description_goes_back_to_its_card_and_the_brief() {
    let mut project = project();
    project.references[2].image = Some("compass.png".into());
    let workshop = Workshop::open(&project, Target::Reference(2)).unwrap();
    assert_eq!(workshop.fields, [(TEXT.to_owned(), "No needle.".to_owned())]);
    assert_eq!(Target::Reference(2).section(&project), "The description an image model draws the reference picture of \"Compass\" (prop) from");
    Target::Reference(2).write(&mut project, &[(TEXT.to_owned(), "Brass, no needle.".to_owned())]);
    project.follow_edited_brief();
    assert_eq!((project.references[2].description.as_str(), project.references[2].image.is_some()), ("Brass, no needle.", true), "the picture stays");
    assert_eq!(project.brief.as_ref().unwrap().props[0].description, "Brass, no needle.");
    assert!(Workshop::open(&project, Target::Reference(9)).is_none());
  }

  #[test]
  fn the_writer_is_told_about_the_rest_of_the_film() {
    let told = context(&project());
    assert!(told.contains("The venue, The Sump Vault: A flooded bunker.") && told.contains("The look: Cold. grainy") && told.contains("Props: Compass"), "{told}");
  }

  fn project() -> SongProject {
    let mut project = SongProject::new();
    project.idea = "A captain with no rudder".to_owned();
    project.visual = Some(Style { statement: "Cold.".into(), block: "grainy".into(), ..Default::default() });
    let subject = |name: &str, role: &str, description: &str| Subject { name: name.into(), role: role.into(), description: description.into() };
    project.set_brief(Brief { title: "Adrift".into(), logline: "L".into(), character: subject("The Captain", "defiant", "Wiry."), venue: subject("The Sump Vault", "oppressive", "A flooded bunker."), props: vec![subject("Compass", "", "No needle.")], ..Default::default() });
    project
  }
}
