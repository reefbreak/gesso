//! System Health: whether each model the app uses can be reached with the user's accounts, and
//! what each account has cost so far.
//!
//! The connections are checked when the page is first opened and again on request; a check
//! generates nothing, so it's free. Costs come from the app's own spend ledger (list prices times
//! what each call used), not from the clouds' bills, which trail by hours.

use egui::{Align2, Color32, FontId, Rect, Sense, Ui, pos2, vec2};

use crate::backend::local::health::{Check, HealthReport, Provider, State};
use crate::backend::local::spend::{Account, ItemSpend, SpendSummary};
use crate::backend::Backend;
use crate::pages::common::Env;
use crate::pages::tools::music_video::video_costs;
use crate::shell::usd;
use crate::theme;
use crate::ui::icons::{self, Icon};
use crate::ui::widgets;

/// The page reads best as a column, not the full width of a wide window.
const COLUMN_WIDTH: f32 = 1040.0;
const ROW_GAP: f32 = 10.0;
/// The connection table's columns, as shares of the row: status, model, used for, detail.
const CONNECTION_COLUMNS: [f32; 4] = [0.12, 0.25, 0.25, 0.38];
/// The cost table's columns: model, calls, today, this month, all time.
const COST_COLUMNS: [f32; 5] = [0.44, 0.11, 0.15, 0.15, 0.15];
/// The music video table's columns: video, all time.
const VIDEO_COLUMNS: [f32; 2] = [0.8, 0.2];
/// How often the music videos' costs are read again.
const VIDEO_COSTS_EVERY_SECS: f64 = 5.0;
/// Space between table columns.
const COLUMN_GUTTER: f32 = 18.0;
const GOOD: Color32 = Color32::from_rgb(0x4a, 0xde, 0x80);

#[derive(Default)]
pub struct HealthPage {
  report: Option<HealthReport>,
  checking: bool,
}

impl HealthPage {
  /// Takes a finished check.
  pub fn on_report(&mut self, report: HealthReport) {
    self.report = Some(report);
    self.checking = false;
  }

  pub fn ui(&mut self, ui: &mut Ui, env: &mut Env<'_>) {
    // The first visit checks without being asked.
    if self.report.is_none() && !self.checking {
      self.check(env);
    }
    let area = ui.max_rect().shrink2(vec2(24.0, 16.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
    let mut again = false;
    egui::ScrollArea::vertical().id_salt("system-health").auto_shrink([false, false]).show(&mut child, |ui| {
      ui.set_width((area.width() - 12.0).min(COLUMN_WIDTH));
      again = self.header(ui);
      let spend = env.backend.spend_report();
      // The headline figures first: they shouldn't sit under two dozen rows of connections.
      cost_totals(ui, spend.as_ref().map(|(summary, _)| summary));
      ui.add_space(28.0);
      cost_videos(ui, env.backend);
      ui.add_space(28.0);
      ui.label(widgets::heading("Model connections", 26.0));
      ui.add_space(14.0);
      match &self.report {
        Some(report) => connections(ui, report),
        None => waiting(ui),
      }
      ui.add_space(28.0);
      cost_models(ui, spend.as_ref().map(|(_, items)| items.as_slice()));
      ui.add_space(24.0);
    });
    if again {
      self.check(env);
    }
  }

  fn check(&mut self, env: &mut Env<'_>) {
    self.checking = true;
    env.backend.check_health();
  }

  /// The title, the summary of what was found, and the button to check again. Returns whether
  /// the button was clicked.
  fn header(&self, ui: &mut Ui) -> bool {
    let mut again = false;
    widgets::hud(ui, "System", theme::FAINT);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
      ui.label(widgets::heading("System health", 34.0));
      ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        again = ui.add_enabled_ui(!self.checking, |ui| widgets::button(ui, Some(Icon::RotateCw), if self.checking { "Checking\u{2026}" } else { "Check again" }, widgets::Kind::Secondary, 0.0, 32.0)).inner.clicked();
        if self.checking {
          let (spinner, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
          icons::paint_spinner(ui, spinner, theme::MUTED);
        } else if let Some(report) = &self.report {
          ui.label(egui::RichText::new(format!("Checked at {}", report.checked_at.format("%H:%M"))).size(12.0).family(theme::mono()).color(theme::FAINT));
        }
      });
    });
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Whether each model can be reached with your own accounts, and what each account has cost. Checking generates nothing, so it's free.").size(13.5).color(theme::MUTED));
    if let Some(report) = &self.report {
      ui.add_space(12.0);
      ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let attention = report.needing_attention();
        tally(ui, report.count(State::Ready), "ready", GOOD);
        tally(ui, report.count(State::Listed), "listed", theme::ACCENT_400);
        tally(ui, report.count(State::NotSetUp), "not set up", theme::MUTED);
        tally(ui, attention, if attention == 1 { "needs attention" } else { "need attention" }, if attention > 0 { theme::BAD } else { theme::MUTED });
      });
    }
    ui.add_space(20.0);
    again
  }
}

/// The placeholder while the first check runs.
fn waiting(ui: &mut Ui) {
  let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 120.0), Sense::hover());
  ui.painter().rect(rect, theme::RADIUS, theme::WASH, theme::hairline(), egui::StrokeKind::Inside);
  icons::paint_spinner(ui, Rect::from_center_size(rect.center() - vec2(0.0, 12.0), vec2(24.0, 24.0)), theme::MUTED);
  ui.painter().text(rect.center() + vec2(0.0, 20.0), Align2::CENTER_CENTER, "Checking your connections\u{2026}", FontId::new(13.0, egui::FontFamily::Proportional), theme::MUTED);
}

/// One table of connections per provider.
fn connections(ui: &mut Ui, report: &HealthReport) {
  for provider in Provider::ALL {
    let checks: Vec<&Check> = report.checks.iter().filter(|c| c.provider == provider).collect();
    if checks.is_empty() {
      continue;
    }
    ui.horizontal(|ui| {
      ui.label(widgets::heading(provider.label(), 20.0));
      if let Some((_, account)) = report.accounts.iter().find(|(p, _)| *p == provider) {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(account).size(12.0).family(theme::mono()).color(theme::MUTED));
      }
    });
    ui.add_space(8.0);
    table(ui, |ui| {
      header_row(ui, &CONNECTION_COLUMNS, &["Status", "Model", "Used for", "What we found"], &[]);
      for check in checks {
        divider(ui);
        row(ui, &CONNECTION_COLUMNS, |ui, column| match column {
          0 => status(ui, check.state),
          1 => {
            ui.label(egui::RichText::new(&check.name).size(13.5).family(theme::medium()).color(theme::INK));
            ui.label(egui::RichText::new(&check.upstream).size(11.0).family(theme::mono()).color(theme::FAINT));
          },
          2 => {
            ui.label(egui::RichText::new(&check.used_for).size(12.5).color(theme::MUTED));
          },
          _ => {
            ui.label(egui::RichText::new(&check.detail).size(12.5).color(if check.state.needs_attention() { theme::INK } else { theme::MUTED }));
          },
        });
      }
    });
    ui.add_space(22.0);
  }
  ui.label(egui::RichText::new("Ready: your account reached the model with a free call. Listed: the provider has the model, but the free check couldn't settle whether your account reaches it.").size(12.0).color(theme::FAINT));
}

/// What each account has cost: today, this month and all time.
fn cost_totals(ui: &mut Ui, summary: Option<&SpendSummary>) {
  ui.label(widgets::heading("Costs by account", 26.0));
  ui.add_space(4.0);
  ui.label(egui::RichText::new("Metered by Gesso as it runs: what each call used, at list prices. Your cloud bills are the final word and trail by hours.").size(13.5).color(theme::MUTED));
  ui.add_space(14.0);
  let Some(summary) = summary else {
    ui.label(egui::RichText::new("Costs are tracked in local mode, where Gesso runs on your own accounts.").size(13.0).color(theme::FAINT));
    return;
  };
  // Six cards, three to a row.
  let width = (ui.available_width() - ROW_GAP * 2.0) / 3.0 - 1.0;
  ui.horizontal_wrapped(|ui| {
    ui.spacing_mut().item_spacing = vec2(ROW_GAP, ROW_GAP);
    total_card(ui, width, "All accounts", summary.today.total(), summary.month.total(), summary.all_time.total());
    for account in Account::ALL {
      total_card(ui, width, account.label(), summary.today.of(account), summary.month.of(account), summary.all_time.of(account));
    }
  });
}

/// Each account's spend, model by model, the costliest first.
fn cost_models(ui: &mut Ui, items: Option<&[ItemSpend]>) {
  let Some(items) = items else {
    return;
  };
  ui.label(widgets::heading("Costs by model", 26.0));
  if items.is_empty() {
    ui.add_space(8.0);
    ui.label(egui::RichText::new("Nothing has been billed yet. Each model appears here after its first call.").size(13.0).color(theme::FAINT));
    return;
  }
  for account in Account::ALL {
    let models: Vec<&ItemSpend> = items.iter().filter(|i| i.account == account).collect();
    if models.is_empty() {
      continue;
    }
    ui.add_space(16.0);
    ui.label(widgets::heading(account.label(), 20.0));
    ui.add_space(8.0);
    table(ui, |ui| {
      header_row(ui, &COST_COLUMNS, &["Model", "Calls", "Today", "This month", "All time"], &[1, 2, 3, 4]);
      for item in models {
        divider(ui);
        row(ui, &COST_COLUMNS, |ui, column| match column {
          0 => {
            ui.label(egui::RichText::new(&item.item).size(12.5).family(theme::mono()).color(theme::INK));
          },
          1 => number(ui, item.calls.to_string(), theme::MUTED),
          2 => number(ui, money(item.today), theme::MUTED),
          3 => number(ui, money(item.month), theme::MUTED),
          _ => number(ui, money(item.all_time), theme::INK),
        });
      }
    });
  }
}

/// What each music video has cost, all time, and all of them together. Read again every few
/// seconds: it means opening every video's project file.
fn cost_videos(ui: &mut Ui, backend: &Backend) {
  let now = ui.input(|i| i.time);
  let id = egui::Id::new("health-video-costs");
  let cached: Option<(f64, Vec<(String, f64)>)> = ui.data(|data| data.get_temp(id));
  let videos = match cached.filter(|(read, _)| now - read < VIDEO_COSTS_EVERY_SECS) {
    Some((_, videos)) => videos,
    None => {
      let videos = video_costs(backend);
      ui.data_mut(|data| data.insert_temp(id, (now, videos.clone())));
      videos
    },
  };
  ui.label(widgets::heading("Costs by music video", 26.0));
  ui.add_space(4.0);
  ui.label(egui::RichText::new("All time, for each video made on the Music Video page. Work done before Gesso counted each video's calls is an estimate from the files in its folder.").size(13.5).color(theme::MUTED));
  ui.add_space(8.0);
  if videos.is_empty() {
    ui.label(egui::RichText::new("No music videos yet.").size(13.0).color(theme::FAINT));
    return;
  }
  table(ui, |ui| {
    header_row(ui, &VIDEO_COLUMNS, &["Video", "All time"], &[1]);
    for (title, cost) in &videos {
      divider(ui);
      row(ui, &VIDEO_COLUMNS, |ui, column| match column {
        0 => {
          ui.label(egui::RichText::new(title).size(12.5).color(theme::INK));
        },
        _ => number(ui, money(*cost), theme::MUTED),
      });
    }
    divider(ui);
    row(ui, &VIDEO_COLUMNS, |ui, column| match column {
      0 => {
        ui.label(egui::RichText::new(if videos.len() == 1 { "Total".to_owned() } else { format!("All {} videos", videos.len()) }).size(12.5).family(theme::semibold()).color(theme::INK));
      },
      _ => number(ui, money(videos.iter().map(|(_, cost)| cost).sum()), theme::INK),
    });
  });
}

/// An account's three totals.
fn total_card(ui: &mut Ui, width: f32, title: &str, today: f64, month: f64, all_time: f64) {
  let (rect, _) = ui.allocate_exact_size(vec2(width, 112.0), Sense::hover());
  let p = ui.painter();
  p.rect(rect, theme::RADIUS, theme::WASH, theme::hairline(), egui::StrokeKind::Inside);
  let left = rect.left() + 14.0;
  p.galley(pos2(left, rect.top() + 12.0), widgets::caps(ui, title, theme::HUD_SIZE, theme::mono(), theme::HUD_SPACING, theme::FAINT), theme::FAINT);
  p.text(pos2(left, rect.top() + 34.0), Align2::LEFT_TOP, money(month), FontId::new(26.0, theme::display()), theme::INK);
  p.text(pos2(left, rect.top() + 66.0), Align2::LEFT_TOP, "this month", FontId::new(11.5, egui::FontFamily::Proportional), theme::FAINT);
  p.text(pos2(left, rect.bottom() - 12.0), Align2::LEFT_BOTTOM, format!("{} today \u{b7} {} all time", money(today), money(all_time)), FontId::new(11.5, theme::mono()), theme::MUTED);
}

/// A bordered block of rows.
fn table(ui: &mut Ui, rows: impl FnOnce(&mut Ui)) {
  egui::Frame::new().fill(theme::WASH).stroke(theme::hairline()).corner_radius(theme::RADIUS).inner_margin(egui::Margin::symmetric(14, 6)).show(ui, |ui| {
    ui.set_width(ui.available_width());
    rows(ui);
  });
}

/// One row: each column gets its share of the width and wraps inside it.
fn row(ui: &mut Ui, columns: &[f32], mut cell: impl FnMut(&mut Ui, usize)) {
  let total = ui.available_width();
  ui.add_space(8.0);
  ui.horizontal_top(|ui| {
    ui.spacing_mut().item_spacing.x = 0.0;
    for (index, share) in columns.iter().enumerate() {
      // The gutter sits between the cells, so wrapped text never touches the next column.
      let width = total * share - COLUMN_GUTTER;
      ui.allocate_ui_with_layout(vec2(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
        ui.set_width(width);
        cell(ui, index);
      });
      ui.add_space(COLUMN_GUTTER);
    }
  });
  ui.add_space(8.0);
}

/// The column titles; `numeric` columns are right-aligned like their figures.
fn header_row(ui: &mut Ui, columns: &[f32], titles: &[&str], numeric: &[usize]) {
  row(ui, columns, |ui, column| {
    let galley = widgets::caps(ui, titles[column], 10.5, theme::mono(), 1.2, theme::FAINT);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), galley.size().y), Sense::hover());
    let x = if numeric.contains(&column) { rect.right() - galley.size().x } else { rect.left() };
    ui.painter().galley(pos2(x, rect.top()), galley, theme::FAINT);
  });
}

fn divider(ui: &mut Ui) {
  let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
  ui.painter().hline(rect.x_range(), rect.center().y, theme::hairline());
}

/// A right-aligned figure.
fn number(ui: &mut Ui, text: String, color: Color32) {
  let galley = ui.painter().layout_no_wrap(text, FontId::new(12.5, theme::mono()), color);
  let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), galley.size().y), Sense::hover());
  ui.painter().galley(pos2(rect.right() - galley.size().x, rect.top()), galley, color);
}

/// A state as a coloured dot and its name.
fn status(ui: &mut Ui, state: State) {
  let color = state_colour(state);
  let galley = ui.painter().layout_no_wrap(state.label().to_owned(), FontId::new(12.5, theme::medium()), color);
  let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), galley.size().y.max(16.0)), Sense::hover());
  ui.painter().circle_filled(pos2(rect.left() + 5.0, rect.center().y), 4.0, color);
  ui.painter().galley(pos2(rect.left() + 16.0, rect.center().y - galley.size().y / 2.0), galley, color);
}

/// A count in the summary line ("14 ready").
fn tally(ui: &mut Ui, count: usize, what: &str, color: Color32) {
  let galley = ui.painter().layout_no_wrap(format!("{count} {what}"), FontId::new(12.5, theme::medium()), color);
  let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(30.0, 12.0), Sense::hover());
  ui.painter().rect(rect, 99.0, theme::WASH, theme::hairline(), egui::StrokeKind::Inside);
  ui.painter().circle_filled(pos2(rect.left() + 12.0, rect.center().y), 3.5, color);
  ui.painter().galley(pos2(rect.left() + 21.0, rect.center().y - galley.size().y / 2.0), galley, color);
}

fn state_colour(state: State) -> Color32 {
  match state {
    State::Ready => GOOD,
    State::Listed => theme::ACCENT_400,
    State::NotSetUp => theme::MUTED,
    State::NoAccess | State::NotFound | State::Failed => theme::BAD,
  }
}

/// Dollars for a table: `$0.00` for nothing, `<$0.01` for a fraction of a cent.
fn money(amount: f64) -> String {
  usd(amount)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn columns_share_the_whole_row() {
    assert!((CONNECTION_COLUMNS.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    assert!((COST_COLUMNS.iter().sum::<f32>() - 1.0).abs() < 1e-6);
  }

  #[test]
  fn problems_are_red_and_working_connections_green() {
    assert_eq!(state_colour(State::Ready), GOOD);
    assert_eq!(state_colour(State::NoAccess), theme::BAD);
    assert_eq!(state_colour(State::NotFound), theme::BAD);
    assert_ne!(state_colour(State::NotSetUp), theme::BAD, "an account that isn't set up isn't a fault");
  }
}
