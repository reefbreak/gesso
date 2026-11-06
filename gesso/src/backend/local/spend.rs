//! The spend meter's ledger: one entry per billed call, saved in `~/Artcraft/local/spend.json`.
//!
//! Each entry is what the app itself spent on the user's accounts, priced at list rates (see
//! [`super::pricing`]), so the totals update the moment a call returns. They're an estimate of
//! this app's usage, not the cloud bill, which also counts anything else on those accounts.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Datelike, Local, Utc};
use log::warn;
use serde::{Deserialize, Serialize};

const FILENAME: &str = "spend.json";

/// Which account a charge lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Account {
  Jev,
  Vertex,
  Bedrock,
  /// fal, with the user's own key (lip sync and vocal separation).
  Fal,
  /// Google's Gemini API with the user's own key and its prepaid balance: songs, and pictures
  /// Vertex had no capacity for.
  GeminiApi,
}

impl Account {
  pub const ALL: [Account; 5] = [Account::Vertex, Account::GeminiApi, Account::Bedrock, Account::Fal, Account::Jev];

  pub fn label(self) -> &'static str {
    match self {
      Account::Jev => "Jev (TypeSafe)",
      Account::Vertex => "Google Cloud",
      Account::Bedrock => "AWS Bedrock",
      Account::Fal => "fal",
      Account::GeminiApi => "Gemini API key",
    }
  }
}

/// One billed call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpendEntry {
  pub at: DateTime<Utc>,
  pub account: Account,
  pub usd: f64,
  /// What was billed: an upstream model id, or `jev`.
  pub item: String,
  #[serde(default)]
  pub maybe_batch_token: Option<String>,
  /// The Music Video project the call was for, and which part of making it.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub maybe_project: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub maybe_part: Option<String>,
}

/// Totals per account.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Totals {
  pub jev: f64,
  pub vertex: f64,
  pub bedrock: f64,
  pub fal: f64,
  pub gemini_api: f64,
}

impl Totals {
  pub fn total(&self) -> f64 {
    self.jev + self.vertex + self.bedrock + self.fal + self.gemini_api
  }

  pub fn of(&self, account: Account) -> f64 {
    match account {
      Account::Jev => self.jev,
      Account::Vertex => self.vertex,
      Account::Bedrock => self.bedrock,
      Account::Fal => self.fal,
      Account::GeminiApi => self.gemini_api,
    }
  }

  fn add(&mut self, account: Account, usd: f64) {
    match account {
      Account::Jev => self.jev += usd,
      Account::Vertex => self.vertex += usd,
      Account::Bedrock => self.bedrock += usd,
      Account::Fal => self.fal += usd,
      Account::GeminiApi => self.gemini_api += usd,
    }
  }
}

/// What the top bar shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpendSummary {
  pub today: Totals,
  pub month: Totals,
  pub all_time: Totals,
  pub generations_today: u32,
  /// The most recent charge: what and how much (a generation's provider and routing combined).
  pub last: Option<(String, f64)>,
}

/// What one model (or `jev`) has cost on one account.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemSpend {
  pub account: Account,
  /// The upstream model id.
  pub item: String,
  /// Billed calls, all time.
  pub calls: u32,
  pub today: f64,
  pub month: f64,
  pub all_time: f64,
}

#[derive(Clone)]
pub struct SpendLedger {
  path: Arc<PathBuf>,
  entries: Arc<Mutex<Vec<SpendEntry>>>,
}

impl SpendLedger {
  pub fn load(library_dir: &Path) -> Self {
    let path = library_dir.join(FILENAME);
    let entries = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).map_err(|err| warn!("Ignoring unreadable {}: {err}", path.display())).ok()).unwrap_or_default();
    Self { path: Arc::new(path), entries: Arc::new(Mutex::new(entries)) }
  }

  /// Adds charges (zero-dollar ones are skipped) and saves.
  pub fn record(&self, new: Vec<SpendEntry>) {
    let new: Vec<SpendEntry> = new.into_iter().filter(|e| e.usd > 0.0).collect();
    if new.is_empty() {
      return;
    }
    let Ok(mut entries) = self.entries.lock() else {
      return;
    };
    entries.extend(new);
    if let Err(err) = save(&self.path, &entries) {
      warn!("Couldn't save {}: {err}", self.path.display());
    }
  }

  pub fn summary(&self) -> SpendSummary {
    let entries = self.entries.lock().map(|e| e.clone()).unwrap_or_default();
    summarize(&entries, Local::now())
  }

  /// Spend per model on each account, the costliest first (System Health).
  pub fn breakdown(&self) -> Vec<ItemSpend> {
    let entries = self.entries.lock().map(|e| e.clone()).unwrap_or_default();
    break_down(&entries, Local::now())
  }

  /// What each Music Video project has cost, part by part (a project id to its parts).
  pub fn by_project(&self) -> BTreeMap<String, Vec<(String, f64)>> {
    let entries = self.entries.lock().map(|e| e.clone()).unwrap_or_default();
    by_project(&entries)
  }

  pub fn path(&self) -> &Path {
    &self.path
  }
}

/// Today and this month in the user's local time.
fn summarize(entries: &[SpendEntry], now: DateTime<Local>) -> SpendSummary {
  let mut summary = SpendSummary::default();
  let mut batches_today: Vec<&str> = Vec::new();
  for entry in entries {
    let local = entry.at.with_timezone(&Local);
    summary.all_time.add(entry.account, entry.usd);
    if local.year() == now.year() && local.month() == now.month() {
      summary.month.add(entry.account, entry.usd);
      if local.day() == now.day() {
        summary.today.add(entry.account, entry.usd);
        if let Some(batch) = entry.maybe_batch_token.as_deref().filter(|b| !batches_today.contains(b)) {
          batches_today.push(batch);
        }
      }
    }
  }
  summary.generations_today = batches_today.len() as u32;
  if let Some(last) = entries.last() {
    // A generation's charges share a batch: show them as one.
    let usd = match &last.maybe_batch_token {
      Some(batch) => entries.iter().filter(|e| e.maybe_batch_token.as_ref() == Some(batch)).map(|e| e.usd).sum(),
      None => last.usd,
    };
    summary.last = Some((last.item.clone(), usd));
  }
  summary
}

/// Charges that name a project, summed by part.
fn by_project(entries: &[SpendEntry]) -> BTreeMap<String, Vec<(String, f64)>> {
  let mut out: BTreeMap<String, Vec<(String, f64)>> = BTreeMap::new();
  for entry in entries {
    let Some(project) = &entry.maybe_project else {
      continue;
    };
    let part = entry.maybe_part.as_deref().unwrap_or("Other");
    let parts = out.entry(project.clone()).or_default();
    match parts.iter_mut().find(|(name, _)| name == part) {
      Some((_, usd)) => *usd += entry.usd,
      None => parts.push((part.to_owned(), entry.usd)),
    }
  }
  out
}

/// Charges grouped by account and model, in the user's local day and month.
fn break_down(entries: &[SpendEntry], now: DateTime<Local>) -> Vec<ItemSpend> {
  let mut items: Vec<ItemSpend> = Vec::new();
  for entry in entries {
    let position = items.iter().position(|i| i.account == entry.account && i.item == entry.item).unwrap_or_else(|| {
      items.push(ItemSpend { account: entry.account, item: entry.item.clone(), calls: 0, today: 0.0, month: 0.0, all_time: 0.0 });
      items.len() - 1
    });
    let item = &mut items[position];
    let local = entry.at.with_timezone(&Local);
    item.calls += 1;
    item.all_time += entry.usd;
    if local.year() == now.year() && local.month() == now.month() {
      item.month += entry.usd;
      if local.day() == now.day() {
        item.today += entry.usd;
      }
    }
  }
  items.sort_by(|a, b| b.all_time.total_cmp(&a.all_time));
  items
}

fn save(path: &Path, entries: &[SpendEntry]) -> anyhow::Result<()> {
  let dir = path.parent().ok_or_else(|| anyhow::anyhow!("no library directory"))?;
  std::fs::create_dir_all(dir)?;
  let mut file = tempfile::NamedTempFile::new_in(dir)?;
  file.write_all(serde_json::to_string(entries)?.as_bytes())?;
  file.persist(path)?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use chrono::TimeZone;

  use super::*;

  #[test]
  fn totals_split_by_day_month_and_account() {
    let now = Local.with_ymd_and_hms(2026, 10, 12, 15, 0, 0).unwrap();
    let at = |d: u32, h: u32| Local.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap().with_timezone(&Utc);
    let entries = vec![entry(Local.with_ymd_and_hms(2026, 9, 30, 9, 0, 0).unwrap().with_timezone(&Utc), Account::Vertex, 5.0, None), entry(at(1, 9), Account::Vertex, 1.00, Some("b0")), entry(at(12, 9), Account::Vertex, 0.134, Some("b1")), entry(at(12, 9), Account::Jev, 0.00002, Some("b1")), entry(at(12, 10), Account::Bedrock, 0.14, Some("b2"))];
    let s = summarize(&entries, now);
    assert!((s.today.total() - 0.27402).abs() < 1e-9);
    assert!((s.month.vertex - 1.134).abs() < 1e-9);
    assert!((s.all_time.total() - 6.27402).abs() < 1e-9);
    assert_eq!(s.generations_today, 2);
    assert_eq!(s.last.as_ref().map(|(item, _)| item.as_str()), Some("stability"));
  }

  #[test]
  fn spend_is_broken_down_by_model_costliest_first() {
    let now = Local.with_ymd_and_hms(2026, 10, 12, 15, 0, 0).unwrap();
    let at = |m: u32, d: u32| Local.with_ymd_and_hms(2026, m, d, 9, 0, 0).unwrap().with_timezone(&Utc);
    let charge = |when, account, usd, item: &str| SpendEntry { at: when, account, usd, item: item.to_owned(), maybe_batch_token: None, maybe_project: None, maybe_part: None };
    let entries = vec![charge(at(9, 30), Account::Vertex, 2.0, "veo"), charge(at(10, 1), Account::Vertex, 1.0, "veo"), charge(at(10, 12), Account::Vertex, 0.5, "veo"), charge(at(10, 12), Account::Vertex, 0.07, "nano"), charge(at(10, 12), Account::Bedrock, 0.14, "nano")];
    let items = break_down(&entries, now);
    assert_eq!(items.iter().map(|i| (i.account, i.item.as_str(), i.calls)).collect::<Vec<_>>(), [(Account::Vertex, "veo", 3), (Account::Bedrock, "nano", 1), (Account::Vertex, "nano", 1)]);
    assert_eq!((items[0].today, items[0].month, items[0].all_time), (0.5, 1.5, 3.5));
  }

  #[test]
  fn a_videos_charges_are_summed_by_part() {
    let tagged = |usd, project: Option<&str>, part: &str| SpendEntry { maybe_project: project.map(str::to_owned), maybe_part: Some(part.to_owned()), ..entry(Utc::now(), Account::Vertex, usd, None) };
    let costs = by_project(&[tagged(0.08, Some("a"), "Song"), tagged(0.5, Some("a"), "Clips"), tagged(0.25, Some("a"), "Clips"), tagged(1.0, Some("b"), "Clips"), tagged(9.0, None, "Clips")]);
    assert_eq!(costs.get("a"), Some(&vec![("Song".to_owned(), 0.08), ("Clips".to_owned(), 0.75)]));
    assert_eq!(costs.get("b"), Some(&vec![("Clips".to_owned(), 1.0)]));
    assert_eq!(costs.len(), 2, "a charge from another page belongs to no video");
    // Ledgers written before videos were counted still load.
    let old: SpendEntry = serde_json::from_str(r#"{"at":"2026-01-01T00:00:00Z","account":"vertex","usd":1.0,"item":"gemini"}"#).unwrap();
    assert_eq!((old.maybe_project, old.maybe_part), (None, None));
  }

  #[test]
  fn charges_survive_a_restart() {
    let dir = std::env::temp_dir().join(format!("artcraft-spend-{}", uuid::Uuid::new_v4()));
    let ledger = SpendLedger::load(&dir);
    ledger.record(vec![entry(Utc::now(), Account::Vertex, 0.067, Some("b")), entry(Utc::now(), Account::Jev, 0.0, Some("b"))]);
    let reloaded = SpendLedger::load(&dir);
    assert!((reloaded.summary().today.vertex - 0.067).abs() < 1e-9);
    assert_eq!(reloaded.entries.lock().unwrap().len(), 1, "zero-dollar charges are skipped");
    let _ = std::fs::remove_dir_all(dir);
  }

  fn entry(at: DateTime<Utc>, account: Account, usd: f64, batch: Option<&str>) -> SpendEntry {
    let item = if account == Account::Bedrock { "stability" } else { "gemini" };
    SpendEntry { at, account, usd, item: item.to_owned(), maybe_batch_token: batch.map(str::to_owned), maybe_project: None, maybe_part: None }
  }
}
