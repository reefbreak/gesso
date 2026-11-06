//! Turns a track's sections and beats into the video's shot slots: where each cut falls and how
//! long each shot runs. The writer model then fills the slots with pictures; the timing here is
//! arithmetic, so cuts always sit on the beat whatever the model says.

use serde::{Deserialize, Serialize};

use super::beats::BeatGrid;
use super::writer::TimedLine;

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

/// How long a song to make. The length is asked for, not set: the lyric writer is told how much
/// to write and Lyria how long to run, and Lyria lands near it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SongLength {
  /// A 30-second piece, from Lyria's clip model.
  Clip,
  Minute,
  NinetySeconds,
  #[default]
  TwoMinutes,
  ThreeMinutes,
}

impl SongLength {
  pub const ALL: [SongLength; 5] = [SongLength::Clip, SongLength::Minute, SongLength::NinetySeconds, SongLength::TwoMinutes, SongLength::ThreeMinutes];

  pub fn label(self) -> &'static str {
    match self {
      SongLength::Clip => "30 s",
      SongLength::Minute => "1 min",
      SongLength::NinetySeconds => "1\u{bd} min",
      SongLength::TwoMinutes => "2 min",
      SongLength::ThreeMinutes => "3 min",
    }
  }

  /// Whether this is the 30-second piece, which its own model makes.
  pub fn is_clip(self) -> bool {
    self == SongLength::Clip
  }

  /// What Lyria is told about the length.
  pub fn music_line(self) -> &'static str {
    match self {
      SongLength::Clip => "A 30-second piece.",
      SongLength::Minute => "A complete song about one minute long, with a short intro and a clear ending.",
      SongLength::NinetySeconds => "A complete song about a minute and a half long, with an intro and a clear ending.",
      SongLength::TwoMinutes => "A complete song about two minutes long, with an intro and an ending.",
      SongLength::ThreeMinutes => "A complete song about three minutes long, with an intro and an ending.",
    }
  }

  /// What the lyric writer is told to aim for: the words set a song's length more than anything.
  pub fn writing_line(self) -> &'static str {
    match self {
      SongLength::Clip => "Aim for a 30-second piece: a single hook or one short section, four lines at most.",
      SongLength::Minute => "Aim for a song of about one minute: a short intro, one verse and one chorus.",
      SongLength::NinetySeconds => "Aim for a song of about a minute and a half: a verse, a chorus, a second verse or a break, and the chorus again.",
      SongLength::TwoMinutes => "Aim for a song of about two minutes: two verses, a chorus sung twice, and a break or a bridge.",
      SongLength::ThreeMinutes => "Aim for a song of about three minutes: two or three verses, a chorus sung three times, a bridge and an outro.",
    }
  }
}

/// How close to a cut a line can start and still count as sung over the next shot.
const LINE_LEAD: f64 = 0.3;

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
    // Without timings, the section's lines are shared out across its shots in order, starting at
    // its first shot: with fewer lines than shots, it is the last shots that go without.
    let mut shared = vec![Vec::new(); count];
    for (k, line) in lines.iter().enumerate() {
      shared[k * count / lines.len()].push(*line);
    }
    for (pair, lines) in cuts.windows(2).zip(shared) {
      out.push(Slot { index: 0, start: pair[0], end: pair[1], section: section.name.clone(), energy: section.energy, lyric: lines.join(" / ") });
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

/// Puts each sung line on the shot it is sung over, by the time it starts (`lines` from listening
/// to the track), replacing the guess `slots` makes from the sections. A line that starts just
/// before a cut is mostly sung after it, so it goes to the next shot. Without timings the guess
/// stays.
pub fn place_lines(slots: &mut [Slot], lines: &[TimedLine]) {
  if slots.is_empty() || lines.iter().all(|line| line.text.trim().is_empty()) {
    return;
  }
  for slot in slots.iter_mut() {
    slot.lyric.clear();
  }
  let last = slots.len() - 1;
  for line in lines.iter().filter(|line| !line.text.trim().is_empty()) {
    let at = line.time + LINE_LEAD;
    let slot = &mut slots[slots.iter().position(|slot| at < slot.end).unwrap_or(last)];
    if !slot.lyric.is_empty() {
      slot.lyric.push_str(" / ");
    }
    slot.lyric.push_str(line.text.trim());
  }
}

/// The names a section of a song goes by, longest first so "pre-chorus" isn't read as "chorus".
const SECTION_WORDS: [&str; 17] = ["instrumental", "pre-chorus", "prechorus", "breakdown", "interlude", "refrain", "bridge", "chorus", "intro", "outro", "verse", "break", "build", "drop", "hook", "solo", "coda"];
/// Words that mark a bracketed line as a direction to the band, not something to sing.
const DIRECTION_WORDS: [&str; 18] = ["repeat", "fade", "instrumental", "guitar", "drum", "bass", "synth", "loop", "beat", "tempo", "bpm", "riff", "kick", "strings", "piano", "silence", "x2", "x3"];

/// Lyrics sorted into what is sung and what is a direction. People write lyrics with notes mixed
/// in ("[Intro: guitar loop, drums kick in]", "Outro [repeat to fade]"), and a music model given
/// them to sing will sing the notes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tidied {
  /// The lyrics with each section headed by a bare tag (`[Verse 1]`) and only sung lines under it.
  pub lyrics: String,
  /// The directions that were taken out, each with the section it belongs to.
  pub directions: Vec<String>,
}

/// Sorts lyrics into sung lines under plain section tags, and directions.
pub fn tidy_lyrics(lyrics: &str) -> Tidied {
  let mut out = Tidied::default();
  let mut lines: Vec<String> = Vec::new();
  let mut section = String::new();
  let mut head = |tag: String, lines: &mut Vec<String>| {
    if !lines.is_empty() {
      lines.push(String::new());
    }
    lines.push(format!("[{tag}]"));
    tag
  };
  for line in lyrics.lines().map(without_markers).filter(|line| !line.is_empty()) {
    let line = line.as_str();
    let bracketed = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')).or_else(|| line.strip_prefix('(').and_then(|l| l.strip_suffix(')')).filter(|inner| inner.contains(':') && section_tag(inner.split(':').next().unwrap_or_default()).is_some()));
    if let Some(inner) = bracketed.map(str::trim) {
      let (label, direction) = inner.split_once(':').map_or((inner, ""), |(label, direction)| (label.trim(), direction.trim()));
      match section_tag(label) {
        // A section label, with or without a note after it ("Intro: guitar loop").
        Some(tag) => {
          let note = if direction.is_empty() && label.eq_ignore_ascii_case(&tag) { String::new() } else if direction.is_empty() { label.to_owned() } else if label.eq_ignore_ascii_case(&tag) { direction.to_owned() } else { format!("{label}, {direction}") };
          section = head(tag, &mut lines);
          if !note.is_empty() {
            out.directions.push(format!("{section}: {note}"));
          }
        },
        // Not a section: a direction if it reads like one, else something shouted or sung.
        None if is_direction(inner) => out.directions.push(if section.is_empty() { inner.to_owned() } else { format!("{section}: {inner}") }),
        None => lines.push(inner.to_owned()),
      }
      continue;
    }
    // A heading without brackets, alone ("Verse 1") or with a note ("Outro [repeat to fade]").
    let (before, note) = match line.find(['[', '(']) {
      Some(at) if line.ends_with([']', ')']) => (line[..at].trim(), line[at + 1..line.len() - 1].trim()),
      _ => (line, ""),
    };
    if before.split_whitespace().count() <= 3 && !before.is_empty() {
      if let Some(tag) = section_tag(before).filter(|tag| heading_only(before, tag)) {
        section = head(tag, &mut lines);
        if !note.is_empty() {
          out.directions.push(format!("{section}: {note}"));
        }
        continue;
      }
    }
    // A sung line: a bracketed aside inside it is a direction ("…all day [shouted]").
    let mut sung = String::new();
    let mut rest = line;
    while let Some((start, end)) = rest.find('[').and_then(|start| rest[start..].find(']').map(|len| (start, start + len))) {
      sung.push_str(&rest[..start]);
      let aside = rest[start + 1..end].trim();
      if !aside.is_empty() {
        out.directions.push(if section.is_empty() { aside.to_owned() } else { format!("{section}: {aside}") });
      }
      rest = &rest[end + 1..];
    }
    sung.push_str(rest);
    let sung = sung.split_whitespace().collect::<Vec<_>>().join(" ");
    if !sung.is_empty() {
      lines.push(sung);
    }
  }
  out.lyrics = lines.join("\n");
  out
}

/// A line without the timing marks a music model puts in its own lyric sheets (`[[A0]]`, `[:]`),
/// which mean nothing to a singer and are sometimes pasted back in with the words.
fn without_markers(line: &str) -> String {
  let mut kept = String::new();
  let mut rest = line.trim();
  while let Some(start) = rest.find('[') {
    let doubled = rest[start..].starts_with("[[");
    let Some(len) = rest[start..].find(if doubled { "]]" } else { "]" }) else { break };
    let end = start + len + if doubled { 2 } else { 1 };
    let inner = rest[start..end].trim_matches(['[', ']']).trim();
    let mark = doubled || !inner.chars().any(char::is_alphabetic);
    kept.push_str(&rest[..if mark { start } else { end }]);
    rest = &rest[end..];
  }
  kept.push_str(rest);
  kept.trim().to_owned()
}

/// The plain tag for a section label: "Short Intro" is `Intro`, "verse 2" is `Verse 2`. `None`
/// when the label names no section.
fn section_tag(label: &str) -> Option<String> {
  let lower = label.to_lowercase();
  let word = SECTION_WORDS.iter().find(|word| lower.split(|c: char| !c.is_alphanumeric() && c != '-').any(|part| part == **word))?;
  let name = match *word {
    "prechorus" | "pre-chorus" => "Pre-Chorus".to_owned(),
    word => format!("{}{}", word[..1].to_uppercase(), &word[1..]),
  };
  let number: String = lower.split_whitespace().find(|part| part.chars().all(|c| c.is_ascii_digit())).unwrap_or_default().to_owned();
  Some(if number.is_empty() { name } else { format!("{name} {number}") })
}

/// Whether an unbracketed line is nothing but a section heading ("Verse 1", "Final chorus"),
/// not a sung line that happens to contain the word.
fn heading_only(line: &str, tag: &str) -> bool {
  let extra = ["final", "last", "first", "second", "third", "short", "long", "big", "double"];
  let tag_words: Vec<String> = tag.to_lowercase().split_whitespace().map(str::to_owned).collect();
  line.to_lowercase().split_whitespace().all(|word| {
    let word = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
    tag_words.iter().any(|t| t == word) || extra.contains(&word) || word == "prechorus"
  })
}

/// Whether a bracketed line that isn't a section label reads as a direction to the band.
fn is_direction(inner: &str) -> bool {
  let lower = inner.to_lowercase();
  inner.contains(':') || DIRECTION_WORDS.iter().any(|word| lower.contains(word))
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

    /// With fewer lines than shots, the lines start on the section's first shot. (They used to
    /// start on its second, so every line sat one shot late.)
    #[test]
    fn a_sections_lines_start_on_its_first_shot() {
      let grid = BeatGrid::steady(120.0, 16.0);
      let mut verse = section("Verse", 0.0, 16.0, Energy::Medium);
      verse.lyrics = "one\ntwo\nthree".to_owned();
      let slots = slots(&grid, &[verse], Pace::Steady);
      assert_eq!(slots.iter().map(|s| s.lyric.as_str()).collect::<Vec<_>>(), ["one", "two", "three", ""]);
    }

    #[test]
    fn timed_lines_go_on_the_shot_they_are_sung_over() {
      let slot = |start: f64, end: f64| Slot { index: 0, start, end, section: "Verse".to_owned(), energy: Energy::Medium, lyric: "a guess".to_owned() };
      let line = |time: f64, text: &str| TimedLine { time, text: text.to_owned() };
      let mut slots = vec![slot(0.0, 4.0), slot(4.0, 8.0), slot(8.0, 12.0)];
      place_lines(&mut slots, &[line(0.5, "first"), line(2.0, "second"), line(7.8, "just before the cut"), line(9.0, " "), line(40.0, "after the end")]);
      assert_eq!(slots.iter().map(|s| s.lyric.as_str()).collect::<Vec<_>>(), ["first / second", "", "just before the cut / after the end"]);
      // No timings: the guess from the sections stays.
      let mut guessed = vec![slot(0.0, 4.0)];
      place_lines(&mut guessed, &[]);
      assert_eq!(guessed[0].lyric, "a guess");
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

    /// Lyrics as people write them, with notes to the band mixed in: the notes come out, the
    /// headings become plain tags, and what is left is what gets sung.
    #[test]
    fn directions_are_taken_out_of_the_lyrics() {
      let written = "Night Bus\n[Intro: guitar loop, drums kick in]\nVerse 1\nFirst sung line\nSecond sung line [shouted]\nHook\nThe hook line\n[Hey! Hey! Hey!]\n[Long instrumental: the loop, drums drop out]\nOutro [repeat to fade]\nThe last line\n(Short Intro: a reprise)";
      let tidied = tidy_lyrics(written);
      assert_eq!(tidied.lyrics, "Night Bus\n\n[Intro]\n\n[Verse 1]\nFirst sung line\nSecond sung line\n\n[Hook]\nThe hook line\nHey! Hey! Hey!\n\n[Instrumental]\n\n[Outro]\nThe last line\n\n[Intro]");
      assert_eq!(tidied.directions, ["Intro: guitar loop, drums kick in", "Verse 1: shouted", "Instrumental: Long instrumental, the loop, drums drop out", "Outro: repeat to fade", "Intro: Short Intro, a reprise"]);
      // Nothing in brackets is left for a singer but the bare tags.
      assert!(tidied.lyrics.lines().filter(|line| line.starts_with('[')).all(|line| !line.contains(':') && line.split_whitespace().count() <= 2));
    }

    #[test]
    fn clean_lyrics_and_sung_lines_are_left_alone() {
      let clean = "[Verse 1]\nA line\n(a backing vocal)\n\n[Pre-Chorus]\nBreak it down for me\n[Chorus]\nThe verse was long";
      let tidied = tidy_lyrics(clean);
      assert_eq!(tidied.lyrics, "[Verse 1]\nA line\n(a backing vocal)\n\n[Pre-Chorus]\nBreak it down for me\n\n[Chorus]\nThe verse was long");
      assert!(tidied.directions.is_empty(), "{:?}", tidied.directions);
      assert_eq!((section_tag("verse 2"), section_tag("Final Chorus"), section_tag("prechorus"), section_tag("Hey! Hey!")), (Some("Verse 2".into()), Some("Chorus".into()), Some("Pre-Chorus".into()), None));
      assert_eq!(tidy_lyrics("[fade out slowly]\n[Whoa]").directions, ["fade out slowly"]);
      // A line copied back from a music model's own lyric sheet, timing marks and all.
      let pasted = tidy_lyrics("[[A0]] [:] (Short Intro: guitar squall loop, breakbeat kicks in)\n[[A1]] [0:12] The first line");
      assert_eq!((pasted.lyrics.as_str(), pasted.directions.as_slice()), ("[Intro]\nThe first line", ["Intro: Short Intro, guitar squall loop, breakbeat kicks in".to_owned()].as_slice()));
      assert_eq!(tidy_lyrics("").lyrics, "");
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
