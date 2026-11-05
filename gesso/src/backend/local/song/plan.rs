//! Turns a track's sections and beats into the video's shot slots: where each cut falls and how
//! long each shot runs. The writer model then fills the slots with pictures; the timing here is
//! arithmetic, so cuts always sit on the beat whatever the model says.

use serde::{Deserialize, Serialize};

use super::beats::BeatGrid;

/// Veo makes clips of these lengths; a shot is generated at the shortest that covers it.
pub const CLIP_SECONDS: [u16; 3] = [4, 6, 8];
/// No shot outlasts the longest clip.
pub const LONGEST_SHOT: f64 = 8.0;
/// Shorter than this and a cut reads as a glitch.
const SHORTEST_SHOT: f64 = 1.0;
/// A section the listener found shorter than this is folded into its neighbour.
const SHORTEST_SECTION: f64 = 1.5;
const BEATS_PER_BAR: usize = 4;

/// How much is going on in a section, which sets how fast it cuts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Energy {
  Low,
  #[default]
  Medium,
  High,
}

impl Energy {
  /// From a model's word for it; anything unrecognised is medium.
  pub fn parse(word: &str) -> Self {
    match word.trim().to_ascii_lowercase().as_str() {
      "low" | "calm" | "quiet" | "sparse" => Energy::Low,
      "high" | "intense" | "peak" | "loud" => Energy::High,
      _ => Energy::Medium,
    }
  }

  /// From a section's name, when nothing better is known.
  pub fn of_section(name: &str) -> Self {
    let name = name.to_ascii_lowercase();
    if ["chorus", "hook", "drop", "refrain"].iter().any(|w| name.contains(w)) {
      Energy::High
    } else if ["intro", "outro", "bridge", "break", "interlude", "instrumental"].iter().any(|w| name.contains(w)) {
      Energy::Low
    } else {
      Energy::Medium
    }
  }
}

/// One stretch of the song.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Section {
  pub name: String,
  pub start: f64,
  pub end: f64,
  pub energy: Energy,
  /// The lines sung in it (empty for instrumental stretches).
  pub lyrics: String,
}

/// How often the video cuts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pace {
  Relaxed,
  #[default]
  Steady,
  Fast,
}

impl Pace {
  pub const ALL: [Pace; 3] = [Pace::Relaxed, Pace::Steady, Pace::Fast];

  pub fn label(self) -> &'static str {
    match self {
      Pace::Relaxed => "Relaxed",
      Pace::Steady => "Steady",
      Pace::Fast => "Fast",
    }
  }

  /// Beats a shot holds for, by the section's energy.
  fn beats(self, energy: Energy) -> usize {
    match (self, energy) {
      (Pace::Relaxed, Energy::Low) => 16,
      (Pace::Relaxed, _) => 8,
      (Pace::Steady, Energy::High) => 4,
      (Pace::Steady, _) => 8,
      (Pace::Fast, Energy::Low) => 8,
      (Pace::Fast, Energy::Medium) => 4,
      (Pace::Fast, Energy::High) => 2,
    }
  }
}

/// One shot's place in the song.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Slot {
  pub index: usize,
  pub start: f64,
  pub end: f64,
  pub section: String,
  pub energy: Energy,
  /// The lines sung during it.
  pub lyric: String,
}

impl Slot {
  pub fn seconds(&self) -> f64 {
    self.end - self.start
  }
}

/// The shortest clip length that covers a shot of `seconds`.
pub fn clip_seconds(seconds: f64) -> u16 {
  CLIP_SECONDS.into_iter().find(|c| f64::from(*c) + 0.01 >= seconds).unwrap_or(CLIP_SECONDS[CLIP_SECONDS.len() - 1])
}

/// The song cut into shots: every cut on a beat, bars kept whole where the tempo allows, faster
/// cutting where the energy is higher. Slots cover the track from 0 to its end without gaps.
pub fn slots(grid: &BeatGrid, sections: &[Section], pace: Pace) -> Vec<Slot> {
  let sections = tidy_sections(sections, grid.duration);
  let period = grid.period();
  let mut out: Vec<Slot> = Vec::new();
  for (i, section) in sections.iter().enumerate() {
    // Cuts between sections move to the nearest beat; the song's own start and end stay put.
    let start = if i == 0 { 0.0 } else { out.last().map_or(0.0, |s| s.end) };
    let end = if i + 1 == sections.len() { grid.duration } else { snap(grid, section.end) };
    if end - start < SHORTEST_SHOT {
      continue;
    }
    let hold = hold_seconds(pace.beats(section.energy), period);
    let cuts = cuts_within(grid, start, end, hold);
    let lines: Vec<&str> = section.lyrics.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let count = cuts.len() - 1;
    for (n, pair) in cuts.windows(2).enumerate() {
      // The section's lines are shared out across its shots in order.
      let (from, to) = (n * lines.len() / count, (n + 1) * lines.len() / count);
      out.push(Slot { index: 0, start: pair[0], end: pair[1], section: section.name.clone(), energy: section.energy, lyric: lines[from..to].join(" / ") });
    }
  }
  if out.is_empty() && grid.duration > 0.0 {
    out.push(Slot { index: 0, start: 0.0, end: grid.duration.min(LONGEST_SHOT), section: "Song".to_owned(), energy: Energy::Medium, lyric: String::new() });
  }
  for (index, slot) in out.iter_mut().enumerate() {
    slot.index = index;
  }
  out
}

/// Sections guessed from the lyrics' `[Verse]`-style tags, sized by how many lines each has. Used
/// when no model has listened to the track.
pub fn sections_from_lyrics(lyrics: &str, duration: f64) -> Vec<Section> {
  let mut parts: Vec<(String, Vec<&str>)> = Vec::new();
  for line in lyrics.lines().map(str::trim).filter(|l| !l.is_empty()) {
    match line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
      Some(tag) => parts.push((tag.trim().to_owned(), Vec::new())),
      None => match parts.last_mut() {
        Some((_, lines)) => lines.push(line),
        None => parts.push(("Verse".to_owned(), vec![line])),
      },
    }
  }
  if parts.is_empty() {
    return vec![Section { name: "Song".to_owned(), start: 0.0, end: duration, energy: Energy::Medium, lyrics: String::new() }];
  }
  // An untagged or instrumental part still takes up a couple of lines' worth of time.
  let weight = |lines: &[&str]| lines.len().max(2) as f64;
  let total: f64 = parts.iter().map(|(_, lines)| weight(lines)).sum();
  let mut at = 0.0;
  parts
    .iter()
    .map(|(name, lines)| {
      let start = at;
      at += duration * weight(lines) / total;
      Section { name: name.clone(), start, end: at, energy: Energy::of_section(name), lyrics: lines.join("\n") }
    })
    .collect()
}

/// Sections in order, clamped to the track, with gaps closed and slivers merged into the section
/// before them.
fn tidy_sections(sections: &[Section], duration: f64) -> Vec<Section> {
  let mut sorted: Vec<Section> = sections.iter().filter(|s| s.end > s.start && s.start < duration).cloned().collect();
  sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
  let mut out: Vec<Section> = Vec::new();
  for mut section in sorted {
    section.start = out.last().map_or(0.0, |prev| prev.end);
    section.end = section.end.min(duration);
    if section.end - section.start < SHORTEST_SECTION {
      if let Some(prev) = out.last_mut() {
        prev.end = prev.end.max(section.end);
        if !section.lyrics.trim().is_empty() {
          prev.lyrics = format!("{}\n{}", prev.lyrics, section.lyrics).trim().to_owned();
        }
        continue;
      }
    }
    out.push(section);
  }
  match out.last_mut() {
    Some(last) => last.end = duration,
    None => out.push(Section { name: "Song".to_owned(), start: 0.0, end: duration, energy: Energy::Medium, lyrics: String::new() }),
  }
  out
}

/// How long a shot of `beats` runs, halved or doubled until it's a usable length.
fn hold_seconds(beats: usize, period: f64) -> f64 {
  if period <= 0.0 {
    return LONGEST_SHOT / 2.0;
  }
  let mut beats = beats.max(1) as f64;
  while beats * period > LONGEST_SHOT && beats > 1.0 {
    beats /= 2.0;
  }
  while beats * period < SHORTEST_SHOT {
    beats *= 2.0;
  }
  (beats * period).min(LONGEST_SHOT)
}

/// Cut times from `start` to `end`, about `hold` seconds apart, each on a beat. A short tail joins
/// the shot before it when the pair still fits in one clip.
fn cuts_within(grid: &BeatGrid, start: f64, end: f64, hold: f64) -> Vec<f64> {
  let mut cuts = vec![start];
  loop {
    let last = cuts[cuts.len() - 1];
    let next = snap(grid, last + hold);
    // Past the end, or so close to it that the tail would be a sliver.
    if next <= last + SHORTEST_SHOT / 2.0 || next >= end - SHORTEST_SHOT {
      break;
    }
    cuts.push(next);
  }
  let last = cuts[cuts.len() - 1];
  if end - last > LONGEST_SHOT {
    // No beat to cut on (an unanalysed or beatless stretch): cut it evenly.
    let pieces = ((end - last) / LONGEST_SHOT).ceil();
    let step = (end - last) / pieces;
    cuts.extend((1..pieces as usize).map(|i| last + i as f64 * step));
  }
  cuts.push(end);
  cuts
}

/// The nearest beat to `time`, preferring a bar start when one is within a beat of it.
fn snap(grid: &BeatGrid, time: f64) -> f64 {
  let nearest = |candidates: &mut dyn Iterator<Item = f64>| candidates.min_by(|a, b| (a - time).abs().total_cmp(&(b - time).abs()));
  let beat = nearest(&mut grid.beats.iter().copied());
  let bar = nearest(&mut grid.beats.iter().skip(grid.downbeat % BEATS_PER_BAR).step_by(BEATS_PER_BAR).copied());
  match (beat, bar) {
    (Some(beat), Some(bar)) if (bar - time).abs() <= grid.period() * 0.75 || (bar - beat).abs() < 1e-6 => bar,
    (Some(beat), _) => beat,
    _ => time,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  mod slots {
    use super::*;

    #[test]
    fn slots_cover_the_song_without_gaps() {
      let grid = BeatGrid::steady(140.0, 172.3);
      let sections = song();
      for pace in Pace::ALL {
        let slots = slots(&grid, &sections, pace);
        assert_eq!(slots[0].start, 0.0);
        assert!((slots.last().unwrap().end - 172.3).abs() < 1e-9);
        for pair in slots.windows(2) {
          assert!((pair[0].end - pair[1].start).abs() < 1e-9, "gap after shot {}", pair[0].index);
        }
        for slot in &slots {
          assert!(slot.seconds() <= LONGEST_SHOT + 1e-6, "{pace:?} shot {} runs {:.2}s", slot.index, slot.seconds());
          assert!(slot.seconds() >= SHORTEST_SHOT / 2.0, "{pace:?} shot {} runs {:.2}s", slot.index, slot.seconds());
        }
        assert_eq!(slots.iter().map(|s| s.index).collect::<Vec<_>>(), (0..slots.len()).collect::<Vec<_>>());
      }
    }

    #[test]
    fn cuts_land_on_beats() {
      let grid = BeatGrid::steady(120.0, 60.0);
      let slots = slots(&grid, &[section("Verse", 0.0, 30.2, Energy::Medium), section("Chorus", 30.2, 60.0, Energy::High)], Pace::Fast);
      for slot in &slots[..slots.len() - 1] {
        let beats = slot.end / 0.5;
        assert!((beats - beats.round()).abs() < 1e-6, "shot {} ends at {:.3}s", slot.index, slot.end);
      }
      // The section change at 30.2 s moves to the bar start at 30 s.
      assert!(slots.iter().any(|s| s.section == "Chorus" && (s.start - 30.0).abs() < 1e-6));
    }

    #[test]
    fn louder_sections_cut_faster() {
      let grid = BeatGrid::steady(120.0, 64.0);
      let slots = slots(&grid, &[section("Verse", 0.0, 32.0, Energy::Medium), section("Chorus", 32.0, 64.0, Energy::High)], Pace::Steady);
      let count = |name: &str| slots.iter().filter(|s| s.section == name).count();
      assert_eq!((count("Verse"), count("Chorus")), (8, 16));
    }

    #[test]
    fn lines_are_shared_out_in_order() {
      let grid = BeatGrid::steady(120.0, 16.0);
      let mut verse = section("Verse", 0.0, 16.0, Energy::Medium);
      verse.lyrics = "one\ntwo\nthree\nfour".to_owned();
      let slots = slots(&grid, &[verse], Pace::Steady);
      assert_eq!(slots.iter().map(|s| s.lyric.as_str()).collect::<Vec<_>>(), ["one", "two", "three", "four"]);
    }

    #[test]
    fn a_track_without_beats_is_still_cut() {
      let grid = BeatGrid { bpm: 0.0, beats: Vec::new(), downbeat: 0, duration: 30.0 };
      let slots = slots(&grid, &[section("Song", 0.0, 30.0, Energy::Medium)], Pace::Steady);
      assert!(slots.len() >= 4);
      assert!(slots.iter().all(|s| s.seconds() <= LONGEST_SHOT + 1e-6));
    }
  }

  mod sections {
    use super::*;

    #[test]
    fn tags_become_sections_sized_by_their_lines() {
      let lyrics = "[Intro]\n[Verse 1]\na\nb\nc\nd\n[Chorus]\ne\nf\n";
      let sections = sections_from_lyrics(lyrics, 80.0);
      assert_eq!(sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Intro", "Verse 1", "Chorus"]);
      assert_eq!(sections.iter().map(|s| s.energy).collect::<Vec<_>>(), [Energy::Low, Energy::Medium, Energy::High]);
      assert!((sections[1].end - sections[1].start - 40.0).abs() < 1e-9);
      assert_eq!(sections[1].lyrics, "a\nb\nc\nd");
      assert!((sections[2].end - 80.0).abs() < 1e-9);
    }

    #[test]
    fn lyrics_without_tags_are_one_verse() {
      let sections = sections_from_lyrics("a\nb", 20.0);
      assert_eq!((sections.len(), sections[0].name.as_str()), (1, "Verse"));
      assert_eq!(sections_from_lyrics("", 20.0)[0].name, "Song");
    }

    #[test]
    fn a_models_sections_are_tidied() {
      // Out of order, a gap, a sliver and an overrun.
      let messy = [section("Chorus", 20.0, 40.5, Energy::High), section("Intro", 0.5, 19.0, Energy::Low), section("Tag", 40.5, 41.0, Energy::Low), section("Outro", 41.0, 99.0, Energy::Low)];
      let tidy = tidy_sections(&messy, 60.0);
      assert_eq!(tidy.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Intro", "Chorus", "Outro"]);
      assert_eq!(tidy[0].start, 0.0);
      assert!((tidy[1].start - tidy[0].end).abs() < 1e-9);
      assert!((tidy[2].end - 60.0).abs() < 1e-9);
    }
  }

  #[test]
  fn clips_are_the_shortest_that_cover_the_shot() {
    assert_eq!([1.7, 4.0, 4.3, 6.0, 7.9, 8.0].map(clip_seconds), [4, 4, 6, 6, 8, 8]);
  }

  fn section(name: &str, start: f64, end: f64, energy: Energy) -> Section {
    Section { name: name.to_owned(), start, end, energy, lyrics: String::new() }
  }

  fn song() -> Vec<Section> {
    vec![section("Intro", 0.0, 13.6, Energy::Low), section("Verse 1", 13.6, 41.2, Energy::Medium), section("Chorus", 41.2, 68.5, Energy::High), section("Verse 2", 68.5, 96.0, Energy::Medium), section("Chorus", 96.0, 123.4, Energy::High), section("Break", 123.4, 144.0, Energy::Low), section("Outro", 144.0, 172.3, Energy::High)]
  }
}
