//! What the writer model is asked at each stage (the song, the creative read, the visual style,
//! the creative brief, the scenes), and how its replies are read. The prompts ask for JSON; the
//! readers take whatever JSON is in the reply, so a model that wraps it in prose or a code fence
//! still works.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::plan::{Energy, Section, Slot};

/// A song on paper: what the music model is given.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
  pub title: String,
  /// The music direction: genre, tempo, instruments, vocal delivery.
  pub style: String,
  /// Lyrics with `[Verse]`-style section tags.
  pub lyrics: String,
}

/// What a listening model makes of the track.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Read {
  /// Two or three words ("frenetic edge").
  pub mood: String,
  pub genre: String,
  /// How the track moves: where it builds, breathes and hits.
  pub summary: String,
}

/// A line of the lyrics and when it's sung.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimedLine {
  pub time: f64,
  pub text: String,
}

/// Everything a listener reported.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Heard {
  pub read: Read,
  pub sections: Vec<Section>,
  pub lines: Vec<TimedLine>,
}

/// A named colour of the palette.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Swatch {
  pub name: String,
  /// `#rrggbb`.
  pub hex: String,
}

/// The look guiding every picture in the video.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
  /// The look in one sentence.
  pub statement: String,
  pub palette: Vec<Swatch>,
  pub lighting: String,
  pub texture: String,
  /// The common style block: the words added to every image and clip prompt.
  pub block: String,
}

/// Someone or something the video shows: the performer, the venue, a prop.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Subject {
  pub name: String,
  /// The performer's role, a venue's mood; empty for props.
  pub role: String,
  /// What the camera sees, specific enough for an image model to draw it the same every time.
  pub description: String,
}

/// The creative idea connecting the track, the performer and the visual world.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Brief {
  pub title: String,
  pub logline: String,
  /// The arc: what drives the performance from start to finish.
  pub drive: String,
  pub vision: String,
  /// What the cuts lock onto.
  pub editing: String,
  pub motifs: Vec<String>,
  pub character: Subject,
  pub venue: Subject,
  pub props: Vec<Subject>,
}

/// What the image and video prompts carry from the style and the brief.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Design {
  /// The common style block.
  pub look: String,
  pub character: String,
  pub venue: String,
}

/// One scene as the writer describes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlannedShot {
  pub index: usize,
  /// The opening frame, for the image model.
  pub image: String,
  /// What moves, for the video model.
  pub motion: String,
  /// The scene's feeling in a few words ("confrontational and manic").
  pub beat: String,
  /// The performer sings the scene's line to the camera.
  pub sing: bool,
}

pub const SONGWRITER: &str = "You are a songwriter and music producer. You write wholly original songs. \
Never quote or closely paraphrase the lyrics of an existing song: when an idea leans on a known song, artist or genre, \
carry over only its general energy, structure and attitude, with entirely new words. \
Describe music by genre, era, tempo, instruments, production and vocal delivery, and never by the name of an artist, band or song, \
because the music model refuses those. Reply with JSON only.";

pub const DIRECTOR: &str = "You are a music video director. You invent original performers and settings: \
no real people, no existing characters, no brand names. You write for image and video models, so you describe what the camera sees, \
concretely, in plain words. Reply with JSON only.";

pub const LISTENER: &str = "You mark up songs for a music video editor. You listen to the whole track and report its mood, where each section starts and ends, and when each line is sung, in seconds. Reply with JSON only.";

/// Asks for a song from an idea, or a rewrite of `current` following `notes`.
pub fn song_prompt(idea: &str, notes: &str, current: Option<&Draft>) -> String {
  let mut prompt = format!("The idea for the song:\n{}\n\n", idea.trim());
  if let Some(draft) = current {
    prompt.push_str(&format!("The current draft, titled \"{}\".\nMusic direction: {}\nLyrics:\n{}\n\n", draft.title.trim(), draft.style.trim(), draft.lyrics.trim()));
  }
  if !notes.trim().is_empty() {
    prompt.push_str(&format!("Notes from the artist, which take priority:\n{}\n\n", notes.trim()));
  }
  prompt.push_str(if current.is_some() { "Rewrite the song following the notes. Keep what they don't mention.\n\n" } else { "Write the song.\n\n" });
  prompt.push_str(
    "Reply with a JSON object with these keys:\n\
     - \"title\": a short title.\n\
     - \"style\": the music direction in two or three sentences: genre and era, tempo in BPM, instruments and production, and how the vocal is delivered. No artist, band or song names.\n\
     - \"lyrics\": the full lyrics, each section headed by a tag on its own line such as [Intro], [Verse 1], [Chorus], [Bridge], [Break] or [Outro]. \
     Instrumental sections are a tag with no lines under it. Aim for a song of two to three minutes.",
  );
  prompt
}

/// Asks what the track is like, where its sections fall and when its lines are sung. Goes with
/// the audio itself.
pub fn listen_prompt(lyrics: &str, duration: f64) -> String {
  let lyrics = if lyrics.trim().is_empty() { "(none supplied)".to_owned() } else { lyrics.trim().to_owned() };
  format!(
    "The attached track is {duration:.1} seconds long. Its lyrics as written (the recording may differ):\n{lyrics}\n\n\
     Listen to the whole track and reply with a JSON object with these keys:\n\
     - \"mood\": the track's mood in two or three words.\n\
     - \"genre\": its genre and close relatives, comma-separated.\n\
     - \"summary\": three or four sentences on how the track moves: how it opens, where the energy builds and peaks, the vocal delivery, how it ends.\n\
     - \"sections\": an array, in order, covering the track from 0 to {duration:.1} seconds with no gaps. Each item has \"name\" (Intro, Verse 1, Chorus, Bridge, Break, Outro and so on), \
     \"start\" and \"end\" in seconds, \"energy\" (\"low\", \"medium\" or \"high\"), and \"lyrics\" (the lines actually sung in that section, one per line; empty when it's instrumental).\n\
     - \"lines\": an array with one item per sung line, in order, each with \"time\" (when the line starts, in seconds) and \"text\"."
  )
}

/// Asks for the visual style. `direction` is the user's own note on the look, when they gave one.
pub fn style_prompt(idea: &str, draft: &Draft, read: &Read, direction: &str) -> String {
  let mut prompt = song_context(idea, draft, read);
  if !direction.trim().is_empty() {
    prompt.push_str(&format!("The artist's direction for the look, which takes priority:\n{}\n\n", direction.trim()));
  }
  prompt.push_str(
    "Design the visual style of the music video. Reply with a JSON object with these keys:\n\
     - \"statement\": the look in one sentence, tied to how the track sounds.\n\
     - \"palette\": four colours, each an object with \"name\" and \"hex\" (#rrggbb).\n\
     - \"lighting\": one sentence.\n\
     - \"texture\": one sentence on film stock, grain and print texture.\n\
     - \"block\": the common style block, one or two sentences of plain description (film stock, grain, contrast, lens, lighting) that will be added to every image and video prompt so all shots match. \
     No story and no subject in it, only how the picture looks.",
  );
  prompt
}

/// Asks for the creative brief: the idea, the performer, the venue and the props.
pub fn brief_prompt(idea: &str, draft: &Draft, read: &Read, style: &Style) -> String {
  format!(
    "{}The visual style: {} {}\n\n\
     Write the creative brief for the music video. Reply with a JSON object with these keys:\n\
     - \"title\": a short working title for the film.\n\
     - \"logline\": one sentence: who does what, where, and how it looks.\n\
     - \"drive\": the arc of the performance in one or two sentences, from how it starts to where it peaks.\n\
     - \"vision\": the director's vision in two or three sentences.\n\
     - \"editing\": what the cuts lock onto (drum hits, vocal consonants, bar lines) and how held shots and fast cuts alternate, in one or two sentences.\n\
     - \"motifs\": three recurring visual motifs, each a short phrase.\n\
     - \"character\": the performer, an object with \"name\", \"role\" and \"description\". The description is two or three sentences on an original person or creature: build, face, hair, clothes and how they move, \
     specific enough that an image model draws the same performer every time.\n\
     - \"venue\": the main location, an object with \"name\", \"role\" (its mood in a few words) and \"description\" (two sentences on what the camera sees there).\n\
     - \"props\": one to three signature props or costume details, each an object with \"name\" and \"description\".",
    song_context(idea, draft, read),
    style.statement.trim(),
    style.block.trim()
  )
}

/// What the writer is told about singing when clips can't be lip-synced.
const NO_SINGING: &str = "The clips are silent and can't lip-sync, so avoid close-ups of the performer singing; prefer movement, gesture, glances at the lens and cutaways.";
/// What it's told when they can: which scenes to sing, and how to frame them so the sync holds.
const SINGING: &str = "Some scenes are lip-synced to the vocal afterwards. Mark a scene \"sing\": true when the performer should sing its line to the camera: about a third to a half of the scenes that have something sung over them, favouring choruses and the lines that matter most, and never a scene with nothing sung. Frame a sung scene as a close-up or medium close-up with the performer's face towards the lens, the mouth clearly visible, well lit and not covered by hands, hair or props, and keep the head fairly steady: the sync breaks when the face turns away or moves fast. Every other scene is \"sing\": false and shows movement, gesture and cutaways, with no one mouthing words.";

/// Asks for a picture and a movement for each slot. With `lip_sync`, the writer also marks the
/// scenes the performer sings to the camera.
pub fn scenes_prompt(brief: &Brief, style: &Style, slots: &[Slot], lip_sync: bool) -> String {
  let mut list = String::new();
  for slot in slots {
    list.push_str(&format!("{}. {} \u{b7} {:.1}s long \u{b7} energy {}", slot.index, slot.section, slot.seconds(), energy_word(slot.energy)));
    if !slot.lyric.is_empty() {
      list.push_str(&format!(" \u{b7} sung: {}", slot.lyric));
    }
    list.push('\n');
  }
  let props: Vec<String> = brief.props.iter().map(|p| format!("{} ({})", p.name.trim(), p.description.trim())).collect();
  format!(
    "The film is \"{}\". {}\nThe arc: {}\nThe director's vision: {}\nEditing: {}\nRecurring motifs: {}\n\
     The performer, {}: {}\nThe venue, {}: {}\nProps: {}\nThe look: {}\n\n\
     The video is cut into {} scenes, listed below with the section each falls in, how long it runs and what is sung over it:\n{list}\n\
     Write every scene. Reply with a JSON object with one key, \"scenes\": an array with one item per scene, in order. Each item has:\n\
     - \"index\": the scene's number from the list.\n\
     - \"image\": the opening frame in one or two sentences: who or what is in it, the framing (wide, medium, close-up, detail), where in the venue, and the light. Don't restate the overall look; it is added to every scene.\n\
     - \"motion\": what happens over the scene in one sentence: the performer's movement and the camera's.\n\
     - \"beat\": the scene's feeling in two to four words.{}\n\n\
     Follow the arc from the first scene to the last. Vary the framing from scene to scene, keep the performer in most of them, and bring the motifs and props back. \
     Match the energy: low is stillness and slow moves, high is fast, abrupt movement. \
     Illustrate what is sung where it helps, without showing words on screen. {}",
    brief.title.trim(),
    brief.logline.trim(),
    brief.drive.trim(),
    brief.vision.trim(),
    brief.editing.trim(),
    brief.motifs.join("; "),
    brief.character.name.trim(),
    brief.character.description.trim(),
    brief.venue.name.trim(),
    brief.venue.description.trim(),
    if props.is_empty() { "none".to_owned() } else { props.join("; ") },
    style.block.trim(),
    slots.len(),
    if lip_sync { "\n- \"sing\": true or false, as described below." } else { "" },
    if lip_sync { SINGING } else { NO_SINGING }
  )
}

pub fn parse_draft(reply: &str) -> anyhow::Result<Draft> {
  let json = find_json(reply).ok_or_else(|| anyhow::anyhow!("The writer didn't reply with a song"))?;
  let lyrics = lines_text(&json["lyrics"]);
  anyhow::ensure!(!lyrics.trim().is_empty(), "The writer's reply had no lyrics");
  Ok(Draft { title: text(&json["title"]), style: text(&json["style"]), lyrics })
}

/// What a listener reported; sections and lines are empty when the reply has none worth using.
pub fn parse_heard(reply: &str) -> Heard {
  let Some(json) = find_json(reply) else {
    return Heard::default();
  };
  let read = Read { mood: text(&json["mood"]), genre: text(&json["genre"]), summary: text(&json["summary"]) };
  let sections = array(&json, "sections")
    .iter()
    .filter_map(|item| {
      let (start, end) = (seconds(&item["start"])?, seconds(&item["end"])?);
      let name = text(&item["name"]);
      let energy = item["energy"].as_str().map_or_else(|| Energy::of_section(&name), Energy::parse);
      (end > start).then(|| Section { name: if name.is_empty() { "Section".to_owned() } else { name }, start, end, energy, lyrics: lines_text(&item["lyrics"]) })
    })
    .collect();
  let lines = json.get("lines").and_then(Value::as_array).into_iter().flatten().filter_map(|item| Some(TimedLine { time: seconds(&item["time"])?, text: text(&item["text"]) })).filter(|line| !line.text.is_empty()).collect();
  Heard { read, sections, lines }
}

pub fn parse_style(reply: &str) -> anyhow::Result<Style> {
  let json = find_json(reply).ok_or_else(|| anyhow::anyhow!("The writer didn't reply with a visual style"))?;
  let palette = json.get("palette").and_then(Value::as_array).into_iter().flatten().filter_map(|item| Some(Swatch { name: text(&item["name"]), hex: hex(&item["hex"])? })).collect();
  let style = Style { statement: text(&json["statement"]), palette, lighting: text(&json["lighting"]), texture: text(&json["texture"]), block: text(&json["block"]) };
  anyhow::ensure!(!style.block.is_empty() || !style.statement.is_empty(), "The writer's reply had no visual style");
  Ok(style)
}

pub fn parse_brief(reply: &str) -> anyhow::Result<Brief> {
  let json = find_json(reply).ok_or_else(|| anyhow::anyhow!("The writer didn't reply with a brief"))?;
  let subject = |value: &Value| Subject { name: text(&value["name"]), role: text(&value["role"]), description: text(&value["description"]) };
  let brief = Brief { title: text(&json["title"]), logline: text(&json["logline"]), drive: text(&json["drive"]), vision: text(&json["vision"]), editing: text(&json["editing"]), motifs: json.get("motifs").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(|m| m.trim().to_owned()).filter(|m| !m.is_empty()).collect(), character: subject(&json["character"]), venue: subject(&json["venue"]), props: json.get("props").and_then(Value::as_array).into_iter().flatten().map(subject).filter(|p| !p.name.is_empty() || !p.description.is_empty()).collect() };
  anyhow::ensure!(!brief.character.description.is_empty(), "The writer's brief had no performer");
  Ok(brief)
}

/// The scenes a writer described, by index. Slots it skipped are left for the caller to fill.
pub fn parse_scenes(reply: &str) -> Vec<PlannedShot> {
  let Some(json) = find_json(reply) else {
    return Vec::new();
  };
  let items = if json.get("scenes").is_some() { array(&json, "scenes") } else { array(&json, "shots") };
  items
    .iter()
    .enumerate()
    .filter_map(|(position, item)| {
      let index = item["index"].as_u64().map_or(position, |i| i as usize);
      let image = text(&item["image"]);
      (!image.is_empty()).then(|| PlannedShot { index, image, motion: text(&item["motion"]), beat: text(&item["beat"]), sing: item["sing"].as_bool().unwrap_or(false) })
    })
    .collect()
}

/// A scene for a slot the writer left out, so the storyboard has no holes.
pub fn fallback_scene(slot: &Slot) -> PlannedShot {
  let (framing, motion, beat) = match slot.energy {
    Energy::Low => ("A wide shot of the performer standing still in the venue", "The camera drifts slowly toward the performer.", "Still and watchful"),
    Energy::Medium => ("A medium shot of the performer moving through the venue", "The performer walks toward the camera, which tracks backward.", "Driving forward"),
    Energy::High => ("A close, low-angle shot of the performer mid-movement", "The performer lunges at the lens with fast, abrupt movement; the handheld camera shakes.", "Confrontational"),
  };
  PlannedShot { index: slot.index, image: format!("{framing}."), motion: motion.to_owned(), beat: beat.to_owned(), sing: false }
}

/// The JSON value in a reply: the whole reply, a fenced block, or the outermost braces or brackets.
pub fn find_json(reply: &str) -> Option<Value> {
  let trimmed = reply.trim();
  if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
    return Some(value);
  }
  let unfenced = trimmed.split("```").nth(1).map(|block| block.trim_start_matches("json").trim());
  if let Some(value) = unfenced.and_then(|block| serde_json::from_str::<Value>(block).ok()) {
    return Some(value);
  }
  [('{', '}'), ('[', ']')].into_iter().find_map(|(open, close)| {
    let (from, to) = (trimmed.find(open)?, trimmed.rfind(close)?);
    (to > from).then(|| serde_json::from_str::<Value>(&trimmed[from..=to]).ok()).flatten()
  })
}

/// The song, the idea behind it and how it sounds, as the director's prompts open.
fn song_context(idea: &str, draft: &Draft, read: &Read) -> String {
  let mut context = format!("The song is \"{}\".\nThe idea behind it: {}\nMusic direction: {}\n", draft.title.trim(), idea.trim(), draft.style.trim());
  if !read.summary.trim().is_empty() {
    context.push_str(&format!("How the finished track sounds ({}; {}): {}\n", read.mood.trim(), read.genre.trim(), read.summary.trim()));
  }
  context.push_str(&format!("Lyrics:\n{}\n\n", draft.lyrics.trim()));
  context
}

/// The array under `key`, or the value itself when the model replied with a bare array.
fn array(json: &Value, key: &str) -> Vec<Value> {
  json.get(key).and_then(Value::as_array).or_else(|| json.as_array()).cloned().unwrap_or_default()
}

fn energy_word(energy: Energy) -> &'static str {
  match energy {
    Energy::Low => "low",
    Energy::Medium => "medium",
    Energy::High => "high",
  }
}

fn text(value: &Value) -> String {
  value.as_str().unwrap_or_default().trim().to_owned()
}

/// Lines as one string, whether the model sent a string or a list.
fn lines_text(value: &Value) -> String {
  match value {
    Value::String(s) => s.trim().to_owned(),
    Value::Array(lines) => lines.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n").trim().to_owned(),
    _ => String::new(),
  }
}

/// `#rrggbb` from a model's colour, or nothing when it isn't one.
fn hex(value: &Value) -> Option<String> {
  let digits = value.as_str()?.trim().trim_start_matches('#').to_ascii_lowercase();
  (digits.len() == 6 && digits.chars().all(|c| c.is_ascii_hexdigit())).then(|| format!("#{digits}"))
}

/// Seconds from a number, a numeric string, or a `m:ss` clock.
fn seconds(value: &Value) -> Option<f64> {
  if let Some(number) = value.as_f64() {
    return Some(number);
  }
  let text = value.as_str()?.trim();
  match text.split_once(':') {
    Some((minutes, rest)) => Some(minutes.trim().parse::<f64>().ok()? * 60.0 + rest.trim().parse::<f64>().ok()?),
    None => text.trim_end_matches('s').trim().parse().ok(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  mod replies {
    use super::*;

    #[test]
    fn json_is_found_however_its_wrapped() {
      let plain = r#"{"title":"T"}"#;
      let fenced = "Here you go:\n```json\n{\"title\":\"T\"}\n```\nEnjoy!";
      let buried = "Sure. {\"title\":\"T\"} Hope that helps.";
      for reply in [plain, fenced, buried] {
        assert_eq!(find_json(reply).unwrap()["title"], "T", "{reply}");
      }
      assert_eq!(find_json("no json here"), None);
    }

    #[test]
    fn a_song_needs_lyrics() {
      let draft = parse_draft(r#"{"title":" Night Shift ","style":"slow","lyrics":["[Verse]","a","b"]}"#).unwrap();
      assert_eq!(draft, Draft { title: "Night Shift".into(), style: "slow".into(), lyrics: "[Verse]\na\nb".into() });
      assert!(parse_draft(r#"{"title":"T","style":"s","lyrics":""}"#).is_err());
      assert!(parse_draft("I can't help with that").is_err());
    }

    #[test]
    fn a_listeners_report_takes_clock_times_and_skips_bad_rows() {
      let reply = r#"{"mood":"frenetic edge","genre":"big beat, breakbeat","summary":"It punches in.",
        "sections":[{"name":"Intro","start":0,"end":"0:12.5","energy":"low","lyrics":""},{"name":"Verse 1","start":"12.5s","end":40,"lyrics":"a\nb"},{"name":"Broken","start":50,"end":45}],
        "lines":[{"time":"0:13","text":"a"},{"time":15.5,"text":"b"},{"time":"x","text":"c"},{"time":20,"text":""}]}"#;
      let heard = parse_heard(reply);
      assert_eq!(heard.read, Read { mood: "frenetic edge".into(), genre: "big beat, breakbeat".into(), summary: "It punches in.".into() });
      assert_eq!(heard.sections.len(), 2);
      assert_eq!((heard.sections[0].end, heard.sections[0].energy), (12.5, Energy::Low));
      assert_eq!((heard.sections[1].start, heard.sections[1].energy, heard.sections[1].lyrics.as_str()), (12.5, Energy::Medium, "a\nb"));
      assert_eq!(heard.lines, [TimedLine { time: 13.0, text: "a".into() }, TimedLine { time: 15.5, text: "b".into() }]);
      assert_eq!(parse_heard("nothing"), Heard::default());
    }

    #[test]
    fn a_style_keeps_only_real_colours() {
      let reply = r##"{"statement":"High-contrast monochrome.","palette":[{"name":"Pitch Black","hex":"#0A0A0A"},{"name":"Stark White","hex":"FFFFFF"},{"name":"Bad","hex":"black"}],"lighting":"Hard single source.","texture":"Heavy grain.","block":"Raw 16mm black-and-white film."}"##;
      let style = parse_style(reply).unwrap();
      assert_eq!(style.palette, [Swatch { name: "Pitch Black".into(), hex: "#0a0a0a".into() }, Swatch { name: "Stark White".into(), hex: "#ffffff".into() }]);
      assert_eq!(style.block, "Raw 16mm black-and-white film.");
      assert!(parse_style(r#"{"palette":[]}"#).is_err());
    }

    #[test]
    fn a_brief_needs_its_performer() {
      let reply = r#"{"title":"Runtime","logline":"L","drive":"D","vision":"V","editing":"E","motifs":["twitching hands"," ",3],
        "character":{"name":"The Orchestrator","role":"lead vocalist","description":"Wiry, cable hair."},
        "venue":{"name":"Rail Vault","role":"claustrophobic","description":"A brick tunnel."},
        "props":[{"name":"Lanyard","description":"A cracked badge."},{}]}"#;
      let brief = parse_brief(reply).unwrap();
      assert_eq!(brief.motifs, ["twitching hands"]);
      assert_eq!((brief.character.name.as_str(), brief.venue.role.as_str(), brief.props.len()), ("The Orchestrator", "claustrophobic", 1));
      assert!(parse_brief(r#"{"title":"T","character":{"name":"X"}}"#).is_err());
    }

    #[test]
    fn scenes_are_matched_by_index() {
      let reply = r#"{"scenes":[{"index":2,"image":"A","motion":"m","beat":"manic","sing":true},{"image":"B"},{"index":5,"image":""}]}"#;
      let scenes = parse_scenes(reply);
      assert_eq!(scenes.iter().map(|s| (s.index, s.image.as_str(), s.beat.as_str(), s.sing)).collect::<Vec<_>>(), [(2, "A", "manic", true), (1, "B", "", false)]);
    }
  }

  mod prompts {
    use super::*;

    #[test]
    fn a_rewrite_carries_the_draft_and_the_notes() {
      let draft = Draft { title: "T".into(), style: "fast".into(), lyrics: "[Verse]\nold line".into() };
      let prompt = song_prompt("an idea", "make the chorus shorter", Some(&draft));
      assert!(prompt.contains("old line") && prompt.contains("make the chorus shorter") && prompt.contains("Rewrite the song"));
      assert!(song_prompt("an idea", "", None).contains("Write the song."));
    }

    #[test]
    fn the_style_follows_the_artists_direction_and_the_read() {
      let draft = Draft { title: "T".into(), style: "fast".into(), lyrics: "la".into() };
      let read = Read { mood: "frenetic".into(), genre: "big beat".into(), summary: "It punches in.".into() };
      let prompt = style_prompt("an idea", &draft, &read, "black and white, a rail tunnel");
      assert!(prompt.contains("How the finished track sounds (frenetic; big beat): It punches in."));
      assert!(prompt.contains("black and white, a rail tunnel") && prompt.contains("takes priority"));
      assert!(!style_prompt("an idea", &draft, &Read::default(), "").contains("takes priority"));
    }

    #[test]
    fn the_scene_list_names_every_slot() {
      let slots = [Slot { index: 0, start: 0.0, end: 3.4, section: "Intro".into(), energy: Energy::Low, lyric: String::new() }, Slot { index: 1, start: 3.4, end: 5.1, section: "Chorus".into(), energy: Energy::High, lyric: "a / b".into() }];
      let brief = Brief { title: "Runtime".into(), props: vec![Subject { name: "Lanyard".into(), role: String::new(), description: "a cracked badge".into() }], ..Default::default() };
      let style = Style { block: "grainy".into(), ..Default::default() };
      let prompt = scenes_prompt(&brief, &style, &slots, false);
      assert!(prompt.contains("can't lip-sync") && !prompt.contains("\"sing\""));
      let singing = scenes_prompt(&brief, &style, &slots, true);
      assert!(singing.contains("- \"sing\": true or false") && singing.contains("the mouth clearly visible") && !singing.contains("can't lip-sync"));
      assert!(prompt.contains("0. Intro \u{b7} 3.4s long \u{b7} energy low\n"));
      assert!(prompt.contains("1. Chorus \u{b7} 1.7s long \u{b7} energy high \u{b7} sung: a / b\n"));
      assert!(prompt.contains("cut into 2 scenes") && prompt.contains("Props: Lanyard (a cracked badge)") && prompt.contains("The look: grainy"));
    }
  }

  #[test]
  fn skipped_slots_get_a_scene_that_fits_their_energy() {
    let slot = Slot { index: 7, energy: Energy::High, ..Default::default() };
    let scene = fallback_scene(&slot);
    assert_eq!(scene.index, 7);
    assert!(scene.motion.contains("lunges"));
  }
}
