//! System Health: whether each model the app uses can be reached with the user's accounts, found
//! out without generating anything, so a check costs nothing.
//!
//! | Models                         | How they're checked                                        |
//! |--------------------------------|------------------------------------------------------------|
//! | Gemini (text and Nano Banana)  | `countTokens` on the project: proves the project may call  |
//! |                                | the model                                                  |
//! | Veo, Lyria, Omni Flash         | Vertex's model catalogue, then an empty request to the     |
//! |                                | model: it's rejected unbilled, which shows the project     |
//! |                                | reached it. Quota only shows on a real generation          |
//! | Stability and Claude (Bedrock) | `GetFoundationModelAvailability`: the account's access     |
//! | FFmpeg, ISNet                  | Looked for on this machine                                 |

use std::future::Future;

use chrono::{DateTime, Local};
use futures::future::join_all;

use super::LocalEngine;
use super::bedrock::{DEFAULT_CLAUDE_MODEL, ModelAccess};
use super::fal::{LIP_SYNC_MODEL, VOCALS_MODEL};
use super::vertex::{OMNI_MODELS, ProbeCall, SONG_CLIP_MODEL, SONG_MODELS, TEXT_MODELS};
use crate::backend::video;

/// Lyria 3 and Omni Flash are served from the global endpoint.
const GLOBAL: &str = "global";

/// Whose account a model runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
  GoogleCloud,
  Aws,
  /// Programs on the user's machine.
  Local,
  /// Other services with their own keys.
  Other,
}

impl Provider {
  pub const ALL: [Provider; 4] = [Provider::GoogleCloud, Provider::Aws, Provider::Local, Provider::Other];

  pub fn label(self) -> &'static str {
    match self {
      Provider::GoogleCloud => "Google Cloud \u{b7} Vertex AI",
      Provider::Aws => "AWS \u{b7} Bedrock",
      Provider::Local => "On this Mac",
      Provider::Other => "Other services",
    }
  }
}

/// What a check found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
  /// Proven to work with the user's account.
  Ready,
  /// In the provider's catalogue, but it couldn't be shown that the account reaches it.
  Listed,
  /// The account or key it needs isn't configured.
  NotSetUp,
  /// The provider refused: no access, or not granted.
  NoAccess,
  /// The provider doesn't know the model (retired, renamed, or not in this region).
  NotFound,
  /// The check itself failed (offline, timed out, credentials expired).
  Failed,
}

impl State {
  pub fn label(self) -> &'static str {
    match self {
      State::Ready => "Ready",
      State::Listed => "Listed",
      State::NotSetUp => "Not set up",
      State::NoAccess => "No access",
      State::NotFound => "Not found",
      State::Failed => "Check failed",
    }
  }

  /// Whether the user has something to fix.
  pub fn needs_attention(self) -> bool {
    matches!(self, State::NoAccess | State::NotFound | State::Failed)
  }
}

/// One connection's status.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
  pub provider: Provider,
  pub name: String,
  /// Which pages rely on it.
  pub used_for: String,
  /// The API model id, or the program.
  pub upstream: String,
  pub state: State,
  /// What was found, or what to do about it.
  pub detail: String,
}

/// Every connection, as of `checked_at`.
#[derive(Clone, Debug, PartialEq)]
pub struct HealthReport {
  pub checked_at: DateTime<Local>,
  pub checks: Vec<Check>,
  /// One line per provider on what's configured (project, region, profile).
  pub accounts: Vec<(Provider, String)>,
}

impl HealthReport {
  pub fn count(&self, state: State) -> usize {
    self.checks.iter().filter(|c| c.state == state).count()
  }

  pub fn needing_attention(&self) -> usize {
    self.checks.iter().filter(|c| c.state.needs_attention()).count()
  }
}

/// How far a check of a model with no free call got.
enum Reach {
  /// Listed, and the model answered the project's empty request.
  Answered,
  /// Listed, but the request didn't settle it: why.
  ListedOnly(String),
}

/// A model to check: its name on screen, what uses it, and its id.
struct Model {
  name: &'static str,
  used_for: &'static str,
  upstream: String,
  /// Other ids the app falls back to when the provider doesn't know `upstream`.
  alternates: Vec<String>,
}

impl Model {
  /// Every id the app would try, in order.
  fn ids(&self) -> impl Iterator<Item = &str> {
    std::iter::once(self.upstream.as_str()).chain(self.alternates.iter().map(String::as_str))
  }
}

impl LocalEngine {
  /// Checks every connection at once. Nothing here generates anything, so nothing is billed.
  pub async fn check_health(&self) -> HealthReport {
    let (google, aws) = futures::join!(self.check_google(), self.check_aws());
    let mut checks = google;
    checks.extend(aws);
    checks.extend(self.check_local());
    checks.extend(self.check_fal());
    checks.push(self.check_jev());
    HealthReport { checked_at: Local::now(), checks, accounts: self.account_lines() }
  }

  async fn check_google(&self) -> Vec<Check> {
    let config = &self.config;
    let gemini = gemini_models(config.song_text_model.as_deref());
    let listed = listed_models(config.song_music_model.as_deref(), config.omni_model.as_deref(), &config.gcp_video_location);
    let check = |model: &Model, state, detail: String| Check { provider: Provider::GoogleCloud, name: model.name.to_owned(), used_for: model.used_for.to_owned(), upstream: model.upstream.clone(), state, detail };
    if !config.vertex_enabled() {
      let why = "Add gcp_project to local_providers.json, then run `gcloud auth application-default login`.";
      return gemini.iter().chain(listed.iter().map(|(model, _, _)| model)).map(|model| check(model, State::NotSetUp, why.to_owned())).collect();
    }
    let sign_in = Model { name: "Google Cloud sign-in", used_for: "Everything on Vertex AI", upstream: "Application Default Credentials".to_owned(), alternates: Vec::new() };
    let mut out = Vec::new();
    // An express-mode key needs no sign-in, and covers Gemini only.
    if config.gcp_project.is_some() {
      match self.vertex.check_sign_in().await {
        Ok(()) => out.push(check(&sign_in, State::Ready, "Credentials found and accepted.".to_owned())),
        Err(err) => {
          // Without a token every other check would fail the same way: say so once.
          let message = format!("{err:#}");
          out.push(check(&sign_in, State::Failed, message));
          out.extend(gemini.iter().chain(listed.iter().map(|(model, _, _)| model)).map(|model| check(model, State::Failed, "Not checked: sign in to Google Cloud first.".to_owned())));
          return out;
        },
      }
    }
    let gemini_results = join_all(gemini.iter().map(|model| first_known(model, |id| self.vertex.check_gemini(id)))).await;
    for (model, (id, result)) in gemini.iter().zip(gemini_results) {
      let (state, detail) = match result {
        Ok(()) => (State::Ready, format!("Your project can call it.{}", fallback_note(model, &id))),
        Err(err) => classify(&format!("{err:#}")),
      };
      out.push(Check { upstream: id, ..check(model, state, detail) });
    }
    if config.gcp_project.is_none() {
      out.extend(listed.iter().map(|(model, _, _)| check(model, State::NotSetUp, "Needs a Google Cloud project: an express-mode key covers Gemini only.".to_owned())));
      return out;
    }
    let listed_results = join_all(listed.iter().map(|(model, location, call)| first_known(model, move |id| self.reach(id, location, *call)))).await;
    for ((model, _, _), (id, result)) in listed.iter().zip(listed_results) {
      let (state, detail) = match result {
        Ok(Reach::Answered) => (State::Ready, format!("Your project reached it: an empty test request was turned down unbilled, as it should be. Quota limits only show on a real generation.{}", fallback_note(model, &id))),
        Ok(Reach::ListedOnly(why)) => (State::Listed, format!("In Vertex's catalogue, but the test request didn't get a clear answer, so access is confirmed the first time it runs. ({why}){}", fallback_note(model, &id))),
        Err(err) => classify(&format!("{err:#}")),
      };
      out.push(Check { upstream: id, ..check(model, state, detail) });
    }
    out
  }

  /// Whether a model with no free call is in Vertex's catalogue and answers the project.
  async fn reach(&self, id: &str, location: &str, call: ProbeCall) -> anyhow::Result<Reach> {
    self.vertex.check_listed(id, location).await?;
    match self.vertex.check_answers(id, location, call).await {
      Ok(()) => Ok(Reach::Answered),
      Err(err) => {
        let text = format!("{err:#}");
        // A refusal or an unknown model is the answer; anything else leaves it at "listed".
        match classify(&text).0 {
          State::NoAccess | State::NotFound => Err(err),
          _ => Ok(Reach::ListedOnly(text.chars().take(160).collect())),
        }
      },
    }
  }

  async fn check_aws(&self) -> Vec<Check> {
    let models = bedrock_models(self.config.song_claude_model.as_deref());
    let check = |model: &Model, state, detail: String| Check { provider: Provider::Aws, name: model.name.to_owned(), used_for: model.used_for.to_owned(), upstream: model.upstream.clone(), state, detail };
    if !self.config.bedrock_enabled() {
      return models.iter().map(|model| check(model, State::NotSetUp, "No AWS credentials were found. Sign in with the AWS CLI (`aws configure` or `aws sso login`).".to_owned())).collect();
    }
    let results = join_all(models.iter().map(|model| self.bedrock.model_access(&model.upstream))).await;
    models
      .iter()
      .zip(results)
      .map(|(model, result)| match result {
        Ok(ModelAccess::Granted) => check(model, State::Ready, format!("Access granted in {}.", self.config.aws_region)),
        Ok(ModelAccess::Blocked(why)) => check(model, State::NoAccess, capitalise(&why)),
        Err(err) => {
          let (state, detail) = classify(&format!("{err:#}"));
          check(model, state, detail)
        },
      })
      .collect()
  }

  fn check_local(&self) -> Vec<Check> {
    let ffmpeg = video::ffmpeg_available();
    let isnet = !self.remover.needs_download();
    vec![Check { provider: Provider::Local, name: "FFmpeg".to_owned(), used_for: "Video previews, Edit Video, Frame Extract, Music Video".to_owned(), upstream: "ffmpeg, ffprobe".to_owned(), state: if ffmpeg { State::Ready } else { State::NotFound }, detail: if ffmpeg { "Installed.".to_owned() } else { "Install it with `brew install ffmpeg`, then restart Gesso.".to_owned() } }, Check { provider: Provider::Local, name: "Background removal model".to_owned(), used_for: "BG Remove".to_owned(), upstream: "isnet-general-use".to_owned(), state: if isnet { State::Ready } else { State::NotSetUp }, detail: if isnet { "Downloaded.".to_owned() } else { "Downloads itself (179 MB) the first time BG Remove runs on your Mac.".to_owned() } }]
  }

  /// fal isn't called: it has no free request, so the rows say only whether a key is saved.
  fn check_fal(&self) -> Vec<Check> {
    let connected = self.fal_connected();
    let (state, detail) = if connected { (State::Listed, "A key is saved. It isn't tested here, because fal has no free call; the first lip sync shows whether it works.") } else { (State::NotSetUp, "Optional: needed only for lip sync. Add your fal API key in Settings.") };
    let row = |name: &str, used_for: &str, upstream: &str| Check { provider: Provider::Other, name: name.to_owned(), used_for: used_for.to_owned(), upstream: upstream.to_owned(), state, detail: detail.to_owned() };
    vec![row("fal lip sync", "Music Video: sung scenes", LIP_SYNC_MODEL), row("fal vocal separation", "Music Video: the vocal stem lip sync follows", VOCALS_MODEL)]
  }

  /// Jev isn't called: a routing call is billed, and a missing key only means Auto uses rules.
  fn check_jev(&self) -> Check {
    let on = self.config.jev_enabled();
    Check { provider: Provider::Other, name: "Jev routing".to_owned(), used_for: "Auto model choice on Image and Video".to_owned(), upstream: self.config.jev_model.clone(), state: if on { State::Listed } else { State::NotSetUp }, detail: if on { "A key is set. It isn't tested here because a routing call is billed.".to_owned() } else { "Optional. Without a key, Auto picks the model by fixed rules.".to_owned() } }
  }

  fn account_lines(&self) -> Vec<(Provider, String)> {
    let c = &self.config;
    let google = match (&c.gcp_project, &c.gcp_api_key) {
      (Some(project), _) => format!("Project {project} \u{b7} Gemini in {} \u{b7} Veo in {}", c.gcp_location, c.gcp_video_location),
      (None, Some(_)) => "Express-mode API key (Gemini only)".to_owned(),
      (None, None) => "No project set".to_owned(),
    };
    let aws = if c.bedrock_enabled() { format!("Region {}{}", c.aws_region, c.aws_profile.as_ref().map(|p| format!(" \u{b7} profile {p}")).unwrap_or_default()) } else { "No credentials found".to_owned() };
    vec![(Provider::GoogleCloud, google), (Provider::Aws, aws)]
  }
}

/// The Gemini models, checked with `countTokens`.
fn gemini_models(text_override: Option<&str>) -> Vec<Model> {
  let model = |name, used_for, upstream: &str| Model { name, used_for, upstream: upstream.to_owned(), alternates: Vec::new() };
  vec![with_fallbacks("Gemini (writer)", "Music Video: lyrics, style, brief, scenes, listening", text_override, &TEXT_MODELS), model("Nano Banana 2", "Image, Edit Image, 3D Stage, Music Video pictures", "gemini-3.1-flash-image"), model("Nano Banana Pro", "Image, Edit Image, Music Video pictures", "gemini-3-pro-image"), model("Nano Banana 2 Lite", "Image", "gemini-3.1-flash-lite-image")]
}

/// The models with no free call: where each runs, and the endpoint the app generates with.
fn listed_models(song_override: Option<&str>, omni_override: Option<&str>, video_location: &str) -> Vec<(Model, String, ProbeCall)> {
  let model = |name, used_for, upstream: &str| Model { name, used_for, upstream: upstream.to_owned(), alternates: Vec::new() };
  let veo = "Video, Edit Video (extend), Music Video shots";
  vec![(model("Veo 3.1 Fast", veo, "veo-3.1-fast-generate-001"), video_location.to_owned(), ProbeCall::PredictLongRunning), (model("Veo 3.1", veo, "veo-3.1-generate-001"), video_location.to_owned(), ProbeCall::PredictLongRunning), (model("Veo 3.1 Lite", veo, "veo-3.1-lite-generate-001"), video_location.to_owned(), ProbeCall::PredictLongRunning), (with_fallbacks("Lyria 3 (songs)", "Music Video song", song_override, &SONG_MODELS), GLOBAL.to_owned(), ProbeCall::Interaction), (model("Lyria 3 Clip", "Music Video 30-second clip", SONG_CLIP_MODEL), GLOBAL.to_owned(), ProbeCall::Interaction), (model("Lyria 2", "Audio", "lyria-002"), video_location.to_owned(), ProbeCall::Predict), (with_fallbacks("Gemini Omni Flash", "Edit Video (describe)", omni_override, &OMNI_MODELS), GLOBAL.to_owned(), ProbeCall::Interaction)]
}

/// The Bedrock models, checked with `GetFoundationModelAvailability`.
fn bedrock_models(claude_override: Option<&str>) -> Vec<Model> {
  let model = |name, used_for, upstream: &str| Model { name, used_for, upstream: upstream.to_owned(), alternates: Vec::new() };
  vec![model("Stable Image Ultra", "Image", "stability.stable-image-ultra-v1:1"), model("SD 3.5 Large", "Image", "stability.sd3-5-large-v1:0"), model("Stable Image Core", "Image", "stability.stable-image-core-v1:1"), model("Stability Inpaint", "Edit Image", "stability.stable-image-inpaint-v1:0"), model("Stability Erase Object", "Edit Image", "stability.stable-image-erase-object-v1:0"), model("Stability Search and Replace", "Edit Image", "stability.stable-image-search-replace-v1:0"), model("Stability Outpaint", "Edit Image", "stability.stable-outpaint-v1:0"), model("Stability Remove Background", "BG Remove", "stability.stable-image-remove-background-v1:0"), model("Claude (writer)", "Music Video, when Claude is the writer", claude_override.unwrap_or(DEFAULT_CLAUDE_MODEL))]
}

/// A model the app tries several ids for: the user's override alone, or the defaults in order.
fn with_fallbacks(name: &'static str, used_for: &'static str, chosen: Option<&str>, defaults: &[&str]) -> Model {
  match chosen {
    Some(id) => Model { name, used_for, upstream: id.to_owned(), alternates: Vec::new() },
    None => Model { name, used_for, upstream: defaults[0].to_owned(), alternates: defaults[1..].iter().map(|id| (*id).to_owned()).collect() },
  }
}

/// Checks a model's ids in the order the app would try them, stopping at the first the provider
/// knows. Returns that id (or the first, when none is known) with its result.
async fn first_known<'a, T, F, Fut>(model: &'a Model, check: F) -> (String, anyhow::Result<T>)
where
  F: Fn(&'a str) -> Fut,
  Fut: Future<Output = anyhow::Result<T>>,
{
  let mut first_failure = None;
  for id in model.ids() {
    match check(id).await {
      Err(err) if classify(&format!("{err:#}")).0 == State::NotFound => {
        first_failure.get_or_insert(err);
      },
      result => return (id.to_owned(), result),
    }
  }
  (model.upstream.clone(), Err(first_failure.unwrap_or_else(|| anyhow::anyhow!("No model id to check"))))
}

/// Says when the id that answered isn't the app's first choice.
fn fallback_note(model: &Model, id: &str) -> String {
  if id == model.upstream {
    String::new()
  } else {
    format!(" {} isn't offered here, so this one is used.", model.upstream)
  }
}

/// What a provider's error means for the user, and what to do about it.
fn classify(error: &str) -> (State, String) {
  let lower = error.to_ascii_lowercase();
  let brief: String = error.chars().take(220).collect();
  if lower.contains("error 404") || lower.contains("not_found") || lower.contains("resourcenotfound") || lower.contains("not found") {
    (State::NotFound, format!("The provider doesn't list this model id here; it may have been renamed or retired. ({brief})"))
  } else if lower.contains("error 403") || lower.contains("permission_denied") || lower.contains("accessdenied") || lower.contains("not authorized") || lower.contains("error 401") {
    (State::NoAccess, format!("The provider refused. Check the API is enabled and your account has the role for it. ({brief})"))
  } else {
    (State::Failed, brief)
  }
}

fn capitalise(text: &str) -> String {
  let mut chars = text.chars();
  match chars.next() {
    Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
    None => String::new(),
  }
}

#[cfg(test)]
mod tests {
  use std::io::{BufRead, BufReader, Read, Write};
  use std::net::TcpListener;
  use std::sync::{Arc, Mutex};

  use super::*;
  use crate::backend::local::config::LocalConfig;

  mod reports {
    use super::*;

    /// With nothing configured, nothing is called and every cloud model says how to set it up.
    #[tokio::test(flavor = "multi_thread")]
    async fn unconfigured_accounts_are_reported_without_calling_anything() {
      let (engine, dir) = engine(LocalConfig { bedrock_enabled: Some(false), ..Default::default() });
      let report = engine.check_health().await;
      let google: Vec<&Check> = report.checks.iter().filter(|c| c.provider == Provider::GoogleCloud).collect();
      assert_eq!(google.len(), 11);
      assert!(google.iter().all(|c| c.state == State::NotSetUp && c.detail.contains("gcp_project")));
      assert!(report.checks.iter().filter(|c| c.provider == Provider::Aws).all(|c| c.state == State::NotSetUp));
      assert_eq!(report.needing_attention(), usize::from(!video::ffmpeg_available()));
      assert_eq!(report.accounts[0], (Provider::GoogleCloud, "No project set".to_owned()));
      let other: Vec<&Check> = report.checks.iter().filter(|c| c.provider == Provider::Other).collect();
      assert_eq!(other.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["fal lip sync", "fal vocal separation", "Jev routing"]);
      assert!(other.iter().all(|c| c.state == State::NotSetUp));
      // With a key saved, fal's rows say so (and that it isn't tested).
      std::fs::create_dir_all(dir.join("credentials")).unwrap();
      std::fs::write(dir.join("credentials").join("fal.api_key.txt"), "key-123\n").unwrap();
      let fal = engine.check_fal();
      assert!(fal.iter().all(|c| c.state == State::Listed && c.detail.contains("isn't tested")));
      let _ = std::fs::remove_dir_all(dir);
    }

    /// With an express-mode key: Gemini is proven with `countTokens` (a model the server doesn't
    /// know is reported as not found), and the project-only models say they need a project.
    #[tokio::test(flavor = "multi_thread")]
    async fn gemini_is_checked_with_count_tokens() {
      let (base_url, requests) = fake_vertex();
      let (engine, dir) = engine(LocalConfig { gcp_api_key: Some("test-key".into()), vertex_base_url: Some(base_url), bedrock_enabled: Some(false), ..Default::default() });
      let report = engine.check_health().await;
      let state_of = |name: &str| report.checks.iter().find(|c| c.name == name).map(|c| c.state).unwrap();
      assert_eq!(state_of("Gemini (writer)"), State::Ready);
      assert_eq!(state_of("Nano Banana 2"), State::Ready);
      assert_eq!(state_of("Nano Banana Pro"), State::NotFound, "the fake server doesn't know it");
      assert_eq!(state_of("Veo 3.1 Fast"), State::NotSetUp);
      // The writer, Nano Banana 2 and Lite, and FFmpeg when it's installed.
      assert_eq!(report.count(State::Ready), 3 + usize::from(video::ffmpeg_available()));
      let requests = requests.lock().unwrap();
      assert_eq!(requests.len(), 4);
      assert!(requests.iter().all(|line| line.starts_with("POST /v1beta1/publishers/google/models/") && line.contains(":countTokens")), "{requests:?}");
      let _ = std::fs::remove_dir_all(dir);
    }
  }

  mod errors {
    use super::*;

    #[test]
    fn provider_errors_say_what_to_do() {
      assert_eq!(classify("Vertex AI error 404 Not Found: Publisher Model `lyria-3.5` was not found").0, State::NotFound);
      assert_eq!(classify("Vertex AI error 403 Forbidden: PERMISSION_DENIED").0, State::NoAccess);
      assert_eq!(classify("Bedrock error: AccessDeniedException: not authorized to perform bedrock:GetFoundationModelAvailability").0, State::NoAccess);
      let (state, detail) = classify("error sending request: connection timed out");
      assert_eq!((state, detail.as_str()), (State::Failed, "error sending request: connection timed out"));
    }

    #[test]
    fn only_real_problems_need_attention() {
      assert!(State::NoAccess.needs_attention() && State::NotFound.needs_attention() && State::Failed.needs_attention());
      assert!(!State::Ready.needs_attention() && !State::Listed.needs_attention() && !State::NotSetUp.needs_attention());
    }
  }

  /// The app falls back through a model's ids, so the check does too: the first id the provider
  /// knows is the one reported, with a note that the first choice isn't offered.
  #[tokio::test]
  async fn the_first_id_the_provider_knows_is_reported() {
    let model = with_fallbacks("Lyria 3 (songs)", "Music Video song", None, &["lyria-new", "lyria-old"]);
    let answer = |known: &'static str| {
      move |id: &str| {
        let found = id == known;
        async move {
          if found {
            Ok(())
          } else {
            Err(anyhow::anyhow!("Vertex AI error 404 Not Found: Publisher Model is not found."))
          }
        }
      }
    };
    let (id, result) = first_known(&model, answer("lyria-old")).await;
    assert_eq!((id.as_str(), result.is_ok()), ("lyria-old", true));
    assert_eq!(fallback_note(&model, &id), " lyria-new isn't offered here, so this one is used.");
    let (id, result) = first_known(&model, answer("lyria-new")).await;
    assert_eq!((id.as_str(), result.is_ok(), fallback_note(&model, &id).as_str()), ("lyria-new", true, ""));
    let (id, result) = first_known(&model, answer("neither")).await;
    assert_eq!(id, "lyria-new");
    assert_eq!(classify(&format!("{:#}", result.unwrap_err())).0, State::NotFound);
    // A refusal isn't a reason to try the next id: it's the answer.
    let (id, result) = first_known(&model, |_| async { Err::<(), _>(anyhow::anyhow!("Vertex AI error 403 Forbidden: PERMISSION_DENIED")) }).await;
    assert_eq!((id.as_str(), result.is_err()), ("lyria-new", true));
  }

  #[test]
  fn overrides_replace_the_default_ids() {
    assert_eq!(gemini_models(Some("gemini-4-argon"))[0].upstream, "gemini-4-argon");
    assert_eq!(gemini_models(None)[0].upstream, TEXT_MODELS[0]);
    let listed = listed_models(Some("lyria-3-pro-preview"), None, "us-central1");
    assert_eq!((listed[3].0.upstream.as_str(), listed[3].1.as_str()), ("lyria-3-pro-preview", "global"));
    assert_eq!((listed[0].1.as_str(), listed[6].0.upstream.as_str()), ("us-central1", OMNI_MODELS[0]));
    assert_eq!((listed[0].2, listed[3].2, listed[5].2), (ProbeCall::PredictLongRunning, ProbeCall::Interaction, ProbeCall::Predict));
    assert!(listed[3].0.alternates.is_empty(), "an override is the only id tried");
    assert_eq!(listed_models(None, None, "us-central1")[3].0.ids().collect::<Vec<_>>(), SONG_MODELS);
    assert_eq!(bedrock_models(Some("us.anthropic.claude-haiku-4-5-20251001-v1:0")).last().unwrap().upstream, "us.anthropic.claude-haiku-4-5-20251001-v1:0");
    assert_eq!(capitalise("not offered in this region"), "Not offered in this region.");
  }

  fn engine(config: LocalConfig) -> (LocalEngine, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("gesso-health-{}", uuid::Uuid::new_v4()));
    (LocalEngine::new(&dir, config, reqwest::Client::builder().no_proxy().build().unwrap()), dir)
  }

  /// Answers `countTokens` for every model but Nano Banana Pro, which it says it doesn't know.
  /// Records each request line.
  fn fake_vertex() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    std::thread::spawn(move || {
      for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let mut length = 0;
        loop {
          let mut line = String::new();
          reader.read_line(&mut line).unwrap();
          if line == "\r\n" {
            break;
          }
          if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap();
          }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let (status, reply) = if request_line.contains("gemini-3-pro-image") { ("404 Not Found", r#"{"error":{"message":"Publisher Model was not found"}}"#) } else { ("200 OK", r#"{"totalTokens":1}"#) };
        record.lock().unwrap().push(request_line.trim().to_owned());
        write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).unwrap();
      }
    });
    (url, seen)
  }
}
