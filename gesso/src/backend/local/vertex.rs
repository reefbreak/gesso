//! Google Vertex AI, with the user's own project: Gemini image and text models
//! (`generateContent`), Veo (`predictLongRunning` + `fetchPredictOperation`), and Gemini Omni
//! Flash video edits and Lyria 3 songs (the Interactions API, `v1beta1/.../interactions`).
//!
//! Auth is Application Default Credentials (`gcloud auth application-default login`, a service
//! account in `GOOGLE_APPLICATION_CREDENTIALS`, or the metadata server). With no project set, an
//! express-mode API key works for Gemini images only.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use gcp_auth::TokenProvider;
use log::{info, warn};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use super::GeneratedFile;
use super::RefImage;
use super::config::LocalConfig;
use super::pricing::{GeminiUsage, OmniUsage};

const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const IMAGE_TIMEOUT: Duration = Duration::from_secs(180);
const VIDEO_POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Veo jobs not done by then are given up on.
const VIDEO_GIVE_UP: Duration = Duration::from_secs(20 * 60);
/// Health checks answer quickly or not at all.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
/// A song made without polling comes back on the request itself.
const SONG_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Omni Flash's id on Vertex (the preview the Vertex samples use), then the Gemini API's GA id.
pub const OMNI_MODELS: [&str; 2] = ["gemini-omni-1.1-flash-preview", "gemini-omni-1.1-flash"];
/// Omni Flash is served from the global endpoint.
const OMNI_LOCATION: &str = "global";
/// Gemini's text models, newest first; the next is tried while Vertex says one isn't found.
pub const TEXT_MODELS: [&str; 2] = ["gemini-3.8-flash", "gemini-3.5-flash"];
/// Lyria's full-song models (songs with vocals, a couple of minutes long). Lyria 3 Pro is the one
/// Vertex lists; Lyria 3.5 is on the Gemini API only so far, and is tried in case it arrives.
pub const SONG_MODELS: [&str; 2] = ["lyria-3-pro-preview", "lyria-3.5"];
/// Full-song models through the Gemini API (a Gemini API key), which has Lyria 3.5.
pub const GEMINI_SONG_MODELS: [&str; 2] = ["lyria-3.5", "lyria-3-pro-preview"];
/// The Gemini API's host, where a Gemini API key makes songs.
const GEMINI_API_HOST: &str = "https://generativelanguage.googleapis.com/";
/// Lyria's 30-second model.
pub const SONG_CLIP_MODEL: &str = "lyria-3-clip-preview";
/// A long reply (a shot list for a whole song) fits in this many tokens.
const TEXT_MAX_OUTPUT_TOKENS: u32 = 32_768;
/// A whole song inline, with the prompt, stays under `generateContent`'s request limit.
const TEXT_TIMEOUT: Duration = Duration::from_secs(300);

/// One Gemini image request (one image comes back per call).
pub struct GeminiImageRequest<'a> {
  pub model: &'a str,
  pub prompt: &'a str,
  pub images: &'a [RefImage],
  /// `16:9` etc.
  pub aspect_ratio: Option<&'a str>,
  /// `1K`, `2K`, `4K`, `512`.
  pub image_size: Option<&'a str>,
}

/// One request to a Gemini text model.
pub struct TextRequest<'a> {
  /// Tried in order while Vertex says a model isn't found.
  pub models: &'a [&'a str],
  pub system: &'a str,
  pub prompt: &'a str,
  /// A track for the model to listen to: its MIME type and bytes.
  pub audio: Option<(&'a str, &'a [u8])>,
  /// Ask for a JSON reply.
  pub json: bool,
  pub temperature: f32,
}

/// A text model's reply, the model that wrote it, and the tokens it reported.
#[derive(Debug)]
pub struct TextReply {
  pub text: String,
  pub model: String,
  pub usage: Option<GeminiUsage>,
}

/// A Lyria 3 song: the audio, whatever text came with it (the lyrics as sung and the song's
/// structure), and the model that made it.
#[derive(Debug)]
pub struct Song {
  pub file: GeneratedFile,
  pub text: String,
  pub model: String,
}

/// Which endpoint a health check sends its empty request to: the one the app generates with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeCall {
  /// `:predict` (Lyria 2).
  Predict,
  /// `:predictLongRunning` (Veo).
  PredictLongRunning,
  /// The Interactions API (Lyria 3, Omni Flash).
  Interaction,
}

/// One Omni Flash edit of a clip (at most 10 s).
pub struct OmniEditRequest<'a> {
  /// Tried in order while Vertex says a model isn't found.
  pub models: &'a [&'a str],
  pub prompt: &'a str,
  /// MP4 bytes.
  pub clip: &'a [u8],
  /// `360p`, `720p`, `1080p`, `4k`.
  pub resolution: &'a str,
  /// Results go to `gs://…` and are downloaded, for clips too big to return inline.
  pub output_gcs_uri: Option<&'a str>,
}

/// An Omni Flash result: the video, the model that made it, and the tokens it reported.
pub struct OmniEdit {
  pub file: GeneratedFile,
  pub model: String,
  pub usage: Option<OmniUsage>,
}

/// One Veo request.
pub struct VeoRequest<'a> {
  pub model: &'a str,
  pub prompt: &'a str,
  pub start_frame: Option<&'a RefImage>,
  pub end_frame: Option<&'a RefImage>,
  pub aspect_ratio: Option<&'a str>,
  pub resolution: Option<&'a str>,
  pub duration_seconds: Option<u16>,
  pub sample_count: u16,
  pub generate_audio: Option<bool>,
  /// A clip to extend (MP4, 1–30 s, 24 fps, 720p or 1080p): Veo continues it by about 7 s.
  pub extend: Option<&'a [u8]>,
}

#[derive(Clone)]
pub struct VertexClient {
  http: reqwest::Client,
  config: LocalConfig,
  auth: Arc<OnceCell<Arc<dyn TokenProvider>>>,
}

impl VertexClient {
  pub fn new(http: reqwest::Client, config: LocalConfig) -> Self {
    Self { http, config, auth: Arc::new(OnceCell::new()) }
  }

  /// Makes one image; an error explains a missing one (usually a safety block).
  pub async fn generate_image(&self, request: GeminiImageRequest<'_>) -> anyhow::Result<GeneratedFile> {
    let response = self.generate_content(request.model, &gemini_image_body(&request), IMAGE_TIMEOUT).await?;
    let mut file = first_image(&response)?;
    file.usage = usage(&response);
    Ok(file)
  }

  /// Asks a Gemini text model, which can also be given a track to listen to.
  pub async fn generate_text(&self, request: TextRequest<'_>) -> anyhow::Result<TextReply> {
    let body = text_body(&request);
    let mut last_error = None;
    for model in request.models {
      match self.generate_content(model, &body, TEXT_TIMEOUT).await {
        Ok(response) => return Ok(TextReply { text: reply_text(&response)?, model: (*model).to_owned(), usage: usage(&response) }),
        Err(err) if is_not_found(&err) => {
          info!("Text model {model} not found on Vertex; trying the next id");
          last_error = Some(err);
        },
        Err(err) => return Err(err),
      }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("No Gemini text model configured")))
  }

  /// The full-song models to try, in order, for the route songs take (see `generate_song`).
  pub fn song_models(&self) -> &'static [&'static str] {
    if self.config.gemini_api_key.is_some() { &GEMINI_SONG_MODELS } else { &SONG_MODELS }
  }

  /// Makes a song with Lyria 3 from a prompt that carries the music direction and the lyrics:
  /// through the Gemini API when a Gemini API key is set, else through Vertex.
  pub async fn generate_song(&self, models: &[&str], prompt: &str) -> anyhow::Result<Song> {
    if let Some(key) = self.config.gemini_api_key.as_deref() {
      return self.generate_song_with_key(key, models, prompt).await;
    }
    let project = self.config.gcp_project.as_deref().ok_or_else(|| anyhow::anyhow!("Lyria needs a Google Cloud project in local_providers.json"))?;
    let base = format!("{}v1beta1/projects/{project}/locations/{OMNI_LOCATION}/interactions", self.host(OMNI_LOCATION));
    let mut last_error = None;
    for model in models {
      // In the background and stored, like Omni Flash: Vertex only lets stored interactions be polled.
      let mut background = true;
      let mut sent = self.post_with_token(&base, &song_body(model, prompt, true), IMAGE_TIMEOUT).await;
      if sent.as_ref().is_err_and(refuses_background) {
        info!("{model} won't run in the background; waiting on the request instead");
        background = false;
        sent = self.post_with_token(&base, &song_body(model, prompt, false), SONG_TIMEOUT).await;
      }
      match sent {
        Ok(started) => {
          let id = started.get("id").and_then(Value::as_str).map(str::to_owned);
          let mut finished = self.wait_for_interaction(&base, started).await;
          if let Some(id) = id {
            self.delete_interaction(&base, &id).await;
          }
          // Vertex sometimes finishes a background song with nothing in it; ask once more and wait.
          if background && finished.as_ref().is_ok_and(came_back_empty) {
            warn!("{model} finished in the background with no output; asking again and waiting on the request");
            finished = self.post_with_token(&base, &song_body(model, prompt, false), SONG_TIMEOUT).await;
          }
          return song_from(&finished?, model, Route::Vertex);
        },
        Err(err) if is_not_found(&err) => {
          info!("Music model {model} not found on Vertex; trying the next id");
          last_error = Some(err);
        },
        Err(err) => return Err(err),
      }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("No Lyria model configured")))
  }

  /// Whether the Gemini API key can see `model` (reading a model's entry isn't billed).
  pub async fn check_gemini_key(&self, model: &str) -> anyhow::Result<()> {
    let key = self.config.gemini_api_key.as_deref().ok_or_else(|| anyhow::anyhow!("No Gemini API key set"))?;
    let url = format!("{}v1beta/models/{model}", self.gemini_host());
    read_json(self.http.get(url).header("x-goog-api-key", key).timeout(PROBE_TIMEOUT).send().await?).await.map(|_| ())
  }

  /// A song through the Gemini API, which answers in one request (nothing is stored there).
  async fn generate_song_with_key(&self, key: &str, models: &[&str], prompt: &str) -> anyhow::Result<Song> {
    let url = format!("{}v1beta/interactions", self.gemini_host());
    let mut last_error = None;
    for model in models {
      let mut body = song_body(model, prompt, false);
      body["store"] = json!(false);
      match read_json(self.http.post(&url).header("x-goog-api-key", key).json(&body).timeout(SONG_TIMEOUT).send().await?).await {
        Ok(finished) => return song_from(&finished, model, Route::GeminiApi),
        Err(err) if is_not_found(&err) => {
          info!("Music model {model} not found on the Gemini API; trying the next id");
          last_error = Some(err);
        },
        Err(err) => return Err(err),
      }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("No Lyria model configured")))
  }

  fn gemini_host(&self) -> String {
    self.config.gemini_base_url.clone().unwrap_or_else(|| GEMINI_API_HOST.to_owned())
  }

  /// Whether Google Cloud credentials can be found and exchanged for a token (no charge).
  pub async fn check_sign_in(&self) -> anyhow::Result<()> {
    self.token().await.map(|_| ())
  }

  /// Whether the project can call a Gemini model: `countTokens`, which Google doesn't bill.
  pub async fn check_gemini(&self, model: &str) -> anyhow::Result<()> {
    let body = json!({ "contents": [{ "role": "user", "parts": [{ "text": "hello" }] }] });
    match (&self.config.gcp_project, &self.config.gcp_api_key) {
      (Some(project), _) => {
        let url = format!("{}v1/projects/{project}/locations/{}/publishers/google/models/{model}:countTokens", self.host(&self.config.gcp_location), self.config.gcp_location);
        self.post_with_token(&url, &body, PROBE_TIMEOUT).await.map(|_| ())
      },
      (None, Some(key)) => {
        let url = format!("{}v1beta1/publishers/google/models/{model}:countTokens", self.host("global"));
        read_json(self.http.post(url).header("x-goog-api-key", key).json(&body).timeout(PROBE_TIMEOUT).send().await?).await.map(|_| ())
      },
      (None, None) => anyhow::bail!("Set a Google Cloud project (or a Vertex API key) in local_providers.json"),
    }
  }

  /// Whether Vertex lists a model in its catalogue at `location` (a read, no charge). Veo and
  /// Lyria have no free call that proves the project may use them, so this is the most that can
  /// be checked without generating something.
  pub async fn check_listed(&self, model: &str, location: &str) -> anyhow::Result<()> {
    let project = self.config.gcp_project.as_deref().ok_or_else(|| anyhow::anyhow!("Needs a Google Cloud project in local_providers.json"))?;
    let url = format!("{}v1/publishers/google/models/{model}", self.host(location));
    let token = self.token().await?;
    read_json(self.http.get(url).bearer_auth(token).header("x-goog-user-project", project).timeout(PROBE_TIMEOUT).send().await?).await.map(|_| ())
  }

  /// Whether a model with no free call answers the project: an empty request is sent to the
  /// endpoint the app generates with, which the model rejects as invalid without running (so
  /// nothing is billed). A rejection for the request's shape means the project reached the model;
  /// a refusal or "not found" comes back as the error it is.
  pub async fn check_answers(&self, model: &str, location: &str, call: ProbeCall) -> anyhow::Result<()> {
    let project = self.config.gcp_project.as_deref().ok_or_else(|| anyhow::anyhow!("Needs a Google Cloud project in local_providers.json"))?;
    let (url, body) = match call {
      ProbeCall::Predict | ProbeCall::PredictLongRunning => {
        let method = if call == ProbeCall::Predict { "predict" } else { "predictLongRunning" };
        (format!("{}v1/projects/{project}/locations/{location}/publishers/google/models/{model}:{method}", self.host(location)), json!({ "instances": [], "parameters": {} }))
      },
      // No input at all, so there's nothing it could generate from.
      ProbeCall::Interaction => (format!("{}v1beta1/projects/{project}/locations/{location}/interactions", self.host(location)), json!({ "model": model })),
    };
    let token = self.token().await?;
    let response = self.http.post(url).bearer_auth(token).json(&body).timeout(PROBE_TIMEOUT).send().await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let message = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_owned)).unwrap_or_else(|| text.chars().take(300).collect());
    if status.is_success() || (status == reqwest::StatusCode::BAD_REQUEST && !rejects_the_model(&message)) {
      return Ok(());
    }
    anyhow::bail!("Vertex AI error {status}: {message}")
  }

  /// One `generateContent` call, with the project's credentials or an express-mode key.
  async fn generate_content(&self, model: &str, body: &Value, timeout: Duration) -> anyhow::Result<Value> {
    match (&self.config.gcp_project, &self.config.gcp_api_key) {
      (Some(project), _) => {
        let url = format!("{}v1/projects/{project}/locations/{}/publishers/google/models/{model}:generateContent", self.host(&self.config.gcp_location), self.config.gcp_location);
        self.post_with_token(&url, body, timeout).await
      },
      (None, Some(key)) => {
        let url = format!("{}v1beta1/publishers/google/models/{model}:generateContent", self.host("global"));
        read_json(self.http.post(url).header("x-goog-api-key", key).json(body).timeout(timeout).send().await?).await
      },
      (None, None) => anyhow::bail!("Set a Google Cloud project (or a Vertex API key) in local_providers.json"),
    }
  }

  /// Starts a Veo job and waits for it, returning every video it made.
  pub async fn generate_video(&self, request: VeoRequest<'_>) -> anyhow::Result<Vec<GeneratedFile>> {
    let project = self.config.gcp_project.as_deref().ok_or_else(|| anyhow::anyhow!("Veo needs a Google Cloud project in local_providers.json"))?;
    let location = &self.config.gcp_video_location;
    let model_path = format!("{}v1/projects/{project}/locations/{location}/publishers/google/models/{}", self.host(location), request.model);
    let started = self.post_with_token(&format!("{model_path}:predictLongRunning"), &veo_body(&request), IMAGE_TIMEOUT).await?;
    let operation = started.get("name").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("Veo didn't return an operation"))?.to_owned();
    info!("Veo operation started: {operation}");
    let deadline = tokio::time::Instant::now() + VIDEO_GIVE_UP;
    loop {
      tokio::time::sleep(VIDEO_POLL_INTERVAL).await;
      let status = match self.post_with_token(&format!("{model_path}:fetchPredictOperation"), &json!({ "operationName": operation }), IMAGE_TIMEOUT).await {
        Ok(status) => status,
        Err(err) if tokio::time::Instant::now() < deadline => {
          warn!("Polling Veo failed (will retry): {err:#}");
          continue;
        },
        Err(err) => return Err(err),
      };
      if !status.get("done").and_then(Value::as_bool).unwrap_or(false) {
        if tokio::time::Instant::now() >= deadline {
          anyhow::bail!("Veo didn't finish in time");
        }
        continue;
      }
      if let Some(message) = status.pointer("/error/message").and_then(Value::as_str) {
        anyhow::bail!("Veo failed: {message}");
      }
      return self.collect_videos(&status).await;
    }
  }

  async fn collect_videos(&self, status: &Value) -> anyhow::Result<Vec<GeneratedFile>> {
    let videos = status.pointer("/response/videos").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out = Vec::new();
    for video in videos {
      if let Some(data) = video.get("bytesBase64Encoded").and_then(Value::as_str) {
        out.push(GeneratedFile { bytes: BASE64.decode(data)?, extension: "mp4", usage: None });
      } else if let Some(uri) = video.get("gcsUri").and_then(Value::as_str) {
        out.push(GeneratedFile { bytes: self.download_gcs(uri).await?, extension: "mp4", usage: None });
      }
    }
    if out.is_empty() {
      let filtered = status.pointer("/response/raiMediaFilteredCount").and_then(Value::as_u64).unwrap_or(0);
      let reasons = status.pointer("/response/raiMediaFilteredReasons").map(Value::to_string).unwrap_or_default();
      anyhow::bail!(if filtered > 0 { format!("Veo filtered the result: {reasons}") } else { "Veo returned no videos".to_owned() });
    }
    Ok(out)
  }

  /// Edits a clip with Gemini Omni Flash and waits for the result.
  pub async fn edit_video(&self, request: OmniEditRequest<'_>) -> anyhow::Result<OmniEdit> {
    let project = self.config.gcp_project.as_deref().ok_or_else(|| anyhow::anyhow!("Gemini Omni Flash needs a Google Cloud project in local_providers.json"))?;
    let base = format!("{}v1beta1/projects/{project}/locations/{OMNI_LOCATION}/interactions", self.host(OMNI_LOCATION));
    let mut last_error = None;
    for model in request.models {
      let mut body = omni_body(model, &request);
      let mut sent = self.post_with_token(&base, &body, IMAGE_TIMEOUT).await;
      // Vertex refuses some output settings for edits; drop the one it names and send again.
      while let Err(err) = &sent {
        let Some(field) = refused_format_field(err, &body) else {
          break;
        };
        warn!("Omni Flash won't take `{field}` for an edit; sending without it");
        if let Some(format) = body.get_mut("response_format").and_then(Value::as_object_mut) {
          format.remove(field);
        }
        sent = self.post_with_token(&base, &body, IMAGE_TIMEOUT).await;
      }
      match sent {
        Ok(started) => {
          let id = started.get("id").and_then(Value::as_str).map(str::to_owned);
          let result = async {
            let finished = self.wait_for_interaction(&base, started).await?;
            let file = self.omni_video(&finished).await?;
            Ok(OmniEdit { file, model: (*model).to_owned(), usage: omni_usage(&finished) })
          }
          .await;
          if let Some(id) = id {
            self.delete_interaction(&base, &id).await;
          }
          return result;
        },
        Err(err) if is_not_found(&err) => {
          info!("Omni model {model} not found on Vertex; trying the next id");
          last_error = Some(err);
        },
        Err(err) => return Err(err),
      }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("No Omni Flash model configured")))
  }

  /// Removes a stored interaction (the uploaded clip and the result) from Google's side.
  async fn delete_interaction(&self, base: &str, id: &str) {
    let Ok(token) = self.token().await else {
      return;
    };
    match self.http.delete(format!("{base}/{id}")).bearer_auth(token).timeout(IMAGE_TIMEOUT).send().await {
      Ok(response) if response.status().is_success() => {},
      Ok(response) => warn!("Couldn't delete Omni interaction {id}: {}", response.status()),
      Err(err) => warn!("Couldn't delete Omni interaction {id}: {err}"),
    }
  }

  /// Polls a background interaction until it has finished.
  async fn wait_for_interaction(&self, base: &str, mut interaction: Value) -> anyhow::Result<Value> {
    let deadline = tokio::time::Instant::now() + VIDEO_GIVE_UP;
    loop {
      match interaction.get("status").and_then(Value::as_str).unwrap_or("completed") {
        "completed" => return Ok(interaction),
        "in_progress" | "queued" => {},
        other => anyhow::bail!("Google reported the request {}: {}", other.replace('_', " "), interaction_error(&interaction).unwrap_or_else(|| "no reason given".to_owned())),
      }
      anyhow::ensure!(tokio::time::Instant::now() < deadline, "Google didn't finish the request in time");
      let id = interaction.get("id").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("Google didn't return an interaction id"))?.to_owned();
      tokio::time::sleep(VIDEO_POLL_INTERVAL).await;
      let token = self.token().await?;
      match read_json(self.http.get(format!("{base}/{id}")).bearer_auth(token).timeout(IMAGE_TIMEOUT).send().await?).await {
        Ok(next) => interaction = next,
        Err(err) if tokio::time::Instant::now() < deadline => warn!("Polling the interaction failed (will retry): {err:#}"),
        Err(err) => return Err(err),
      }
    }
  }

  /// The video in a finished interaction: inline bytes or a `gs://` file.
  async fn omni_video(&self, interaction: &Value) -> anyhow::Result<GeneratedFile> {
    match find_video(interaction) {
      Some(VideoOut::Inline(data)) => Ok(GeneratedFile { bytes: BASE64.decode(data)?, extension: "mp4", usage: None }),
      Some(VideoOut::Uri(uri)) if uri.starts_with("gs://") => Ok(GeneratedFile { bytes: self.download_gcs(&uri).await?, extension: "mp4", usage: None }),
      Some(VideoOut::Uri(uri)) => anyhow::bail!("Omni Flash returned a video at {uri}, which Gesso can't download"),
      None => anyhow::bail!("Omni Flash returned no video{}", interaction_error(interaction).map(|e| format!(": {e}")).unwrap_or_default()),
    }
  }

  /// Makes `count` music clips with Lyria (`predict`, in the video region).
  pub async fn generate_music(&self, model: &str, prompt: &str, negative_prompt: Option<&str>, count: u16) -> anyhow::Result<Vec<GeneratedFile>> {
    let project = self.config.gcp_project.as_deref().ok_or_else(|| anyhow::anyhow!("Lyria needs a Google Cloud project in local_providers.json"))?;
    let location = &self.config.gcp_video_location;
    let url = format!("{}v1/projects/{project}/locations/{location}/publishers/google/models/{model}:predict", self.host(location));
    let response = self.post_with_token(&url, &lyria_body(prompt, negative_prompt, count), IMAGE_TIMEOUT).await?;
    let clips: Vec<GeneratedFile> = response.get("predictions").and_then(Value::as_array).into_iter().flatten().filter_map(|p| p.get("bytesBase64Encoded").and_then(Value::as_str)).filter_map(|data| BASE64.decode(data).ok()).map(|bytes| GeneratedFile { bytes, extension: "wav", usage: None }).collect();
    anyhow::ensure!(!clips.is_empty(), "Lyria returned no audio (the prompt may have been blocked)");
    Ok(clips)
  }

  /// Reads `gs://bucket/object` with the same credentials.
  async fn download_gcs(&self, uri: &str) -> anyhow::Result<Vec<u8>> {
    let (bucket, object) = uri.strip_prefix("gs://").and_then(|rest| rest.split_once('/')).ok_or_else(|| anyhow::anyhow!("Unexpected GCS URI {uri}"))?;
    let object: String = url::form_urlencoded::byte_serialize(object.as_bytes()).collect();
    let url = format!("https://storage.googleapis.com/storage/v1/b/{bucket}/o/{object}?alt=media");
    let token = self.token().await?;
    let response = self.http.get(url).bearer_auth(token).timeout(IMAGE_TIMEOUT).send().await?.error_for_status()?;
    Ok(response.bytes().await?.to_vec())
  }

  /// The configured override, else the location's real host.
  fn host(&self, location: &str) -> String {
    self.config.vertex_base_url.clone().unwrap_or_else(|| host(location))
  }

  async fn post_with_token(&self, url: &str, body: &Value, timeout: Duration) -> anyhow::Result<Value> {
    let token = self.token().await?;
    read_json(self.http.post(url).bearer_auth(token).json(body).timeout(timeout).send().await?).await
  }

  async fn token(&self) -> anyhow::Result<String> {
    let provider = self.auth.get_or_try_init(|| async { gcp_auth::provider().await.map_err(|err| anyhow::anyhow!("Google Cloud credentials not found ({err}). Run `gcloud auth application-default login`.")) }).await?;
    Ok(provider.token(&[SCOPE]).await.map_err(|err| anyhow::anyhow!("Couldn't get a Google Cloud token: {err}"))?.as_str().to_owned())
  }
}

/// The API host for a location: `global`, a multi-region (`us`, `eu`), or a region.
fn host(location: &str) -> String {
  match location {
    "global" => "https://aiplatform.googleapis.com/".to_owned(),
    "us" | "eu" => format!("https://aiplatform.{location}.rep.googleapis.com/"),
    region => format!("https://{region}-aiplatform.googleapis.com/"),
  }
}

fn gemini_image_body(request: &GeminiImageRequest<'_>) -> Value {
  let mut parts: Vec<Value> = request.images.iter().map(|image| json!({ "inlineData": { "mimeType": image.mime_type, "data": BASE64.encode(&image.bytes) } })).collect();
  parts.push(json!({ "text": request.prompt }));
  let mut image_config = serde_json::Map::new();
  if let Some(ratio) = request.aspect_ratio {
    image_config.insert("aspectRatio".into(), json!(ratio));
  }
  // 2.5 Flash Image has no size setting.
  if let Some(size) = request.image_size.filter(|_| !request.model.starts_with("gemini-2.5")) {
    image_config.insert("imageSize".into(), json!(size));
  }
  json!({
    "contents": [{ "role": "user", "parts": parts }],
    "generationConfig": { "responseModalities": ["TEXT", "IMAGE"], "imageConfig": image_config },
  })
}

/// The Interactions API request for an Omni Flash edit.
fn omni_body(model: &str, request: &OmniEditRequest<'_>) -> Value {
  // No aspect ratio: an edit keeps the clip's ("cannot be set in response format for edit task").
  let mut format = json!({ "type": "video", "resolution": request.resolution, "delivery": "inline" });
  if let Some(uri) = request.output_gcs_uri {
    format["delivery"] = json!("uri");
    format["gcs_uri"] = json!(uri);
  }
  json!({
    "model": model,
    "input": [
      { "type": "video", "mime_type": "video/mp4", "data": BASE64.encode(request.clip) },
      { "type": "text", "text": request.prompt },
    ],
    "generation_config": { "video_config": { "task": "edit" } },
    "response_format": format,
    // Background interactions must be stored to be polled; ArtCraft deletes it once it has the video.
    "background": true,
    "store": true,
  })
}

/// A `generateContent` request for text: an optional track first, then the prompt.
fn text_body(request: &TextRequest<'_>) -> Value {
  let mut parts: Vec<Value> = Vec::new();
  if let Some((mime_type, bytes)) = request.audio {
    parts.push(json!({ "inlineData": { "mimeType": mime_type, "data": BASE64.encode(bytes) } }));
  }
  parts.push(json!({ "text": request.prompt }));
  let mut generation = serde_json::Map::new();
  generation.insert("temperature".into(), json!(request.temperature));
  generation.insert("maxOutputTokens".into(), json!(TEXT_MAX_OUTPUT_TOKENS));
  if request.json {
    generation.insert("responseMimeType".into(), json!("application/json"));
  }
  json!({
    "systemInstruction": { "parts": [{ "text": request.system }] },
    "contents": [{ "role": "user", "parts": parts }],
    "generationConfig": generation,
  })
}

/// The text of a `generateContent` reply, without the model's thoughts.
fn reply_text(response: &Value) -> anyhow::Result<String> {
  let candidates = response.get("candidates").and_then(Value::as_array).cloned().unwrap_or_default();
  let text: String = candidates.first().and_then(|c| c.pointer("/content/parts")).and_then(Value::as_array).into_iter().flatten().filter(|p| !p.get("thought").and_then(Value::as_bool).unwrap_or(false)).filter_map(|p| p.get("text").and_then(Value::as_str)).collect();
  if !text.trim().is_empty() {
    return Ok(text);
  }
  let reason = candidates.first().and_then(|c| c.get("finishReason")).and_then(Value::as_str).or_else(|| response.pointer("/promptFeedback/blockReason").and_then(Value::as_str));
  anyhow::bail!("The model returned no text{}", reason.map(|r| format!(" ({r})")).unwrap_or_default())
}

/// The Interactions API request for a Lyria song.
fn song_body(model: &str, prompt: &str, background: bool) -> Value {
  let mut body = json!({ "model": model, "input": prompt });
  if background {
    body["background"] = json!(true);
    body["store"] = json!(true);
  }
  body
}

/// Whether a "bad request" is about the model itself (unknown or not offered), not the empty
/// request's shape.
fn rejects_the_model(message: &str) -> bool {
  let lower = message.to_ascii_lowercase();
  ["not found", "not supported", "unsupported model", "unknown model", "invalid model", "does not exist", "not available", "not allowed", "permission"].iter().any(|phrase| lower.contains(phrase))
}

/// Whether Vertex refused the request for asking to run in the background.
fn refuses_background(err: &anyhow::Error) -> bool {
  let text = err.to_string().to_ascii_lowercase();
  text.contains("error 400") && text.contains("background")
}

/// Content blocks of `kind` (`audio`, `video`, `text`) in an interaction's model output.
fn output_blocks<'a>(interaction: &'a Value, kind: &'a str) -> impl Iterator<Item = &'a Value> {
  let steps = interaction.get("steps").and_then(Value::as_array).into_iter().flatten();
  steps.filter(|s| s.get("type").and_then(Value::as_str).is_none_or(|t| t == "model_output")).flat_map(|s| s.get("content").and_then(Value::as_array).into_iter().flatten()).filter(move |c| c.get("type").and_then(Value::as_str) == Some(kind))
}

/// Which Google API made a song; the advice differs when one comes back without audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
  Vertex,
  GeminiApi,
}

/// The song and its text in a finished interaction.
fn song_from(finished: &Value, model: &str, route: Route) -> anyhow::Result<Song> {
  let file = song_audio(finished, route)?;
  Ok(Song { file, text: interaction_text(finished), model: model.to_owned() })
}

/// The song in a finished interaction (the last audio block, as the SDK's `output_audio` is).
fn song_audio(interaction: &Value, route: Route) -> anyhow::Result<GeneratedFile> {
  let block = output_blocks(interaction, "audio").last().or_else(|| interaction.get("output_audio"));
  let Some(data) = block.and_then(|b| b.get("data")).and_then(Value::as_str) else {
    warn!("A song came back without audio; the reply had: {}", reply_shape(interaction));
    anyhow::bail!("{}", no_audio_reason(interaction, route));
  };
  let bytes = BASE64.decode(data)?;
  let mime = block.and_then(|b| b.get("mime_type")).and_then(Value::as_str).unwrap_or_default();
  let extension = if mime.contains("wav") || bytes.starts_with(b"RIFF") { "wav" } else { "mp3" };
  Ok(GeneratedFile { bytes, extension, usage: None })
}

/// Why a finished song has no audio: Google's error, Lyria's own words, or (a bare "completed"
/// with no output at all) a fault on Google's side that the lyrics didn't cause.
fn no_audio_reason(interaction: &Value, route: Route) -> String {
  if let Some(error) = interaction_error(interaction) {
    return format!("Lyria returned no audio: {error}");
  }
  let said = interaction_text(interaction);
  if !said.is_empty() {
    return format!("Lyria returned no audio, so the prompt or lyrics were probably blocked. It said: {}", said.chars().take(300).collect::<String>());
  }
  match (came_back_empty(interaction), route) {
    (true, Route::Vertex) => "Vertex finished the request but sent no song back, twice. That is a fault on Google's side with Lyria 3 on Vertex, not your lyrics. Add a Gemini API key (System Health says how) to make songs through the Gemini API instead, or import a track.".to_owned(),
    (true, Route::GeminiApi) => "The Gemini API finished the request but sent no song back. That is a fault on Google's side, not your lyrics; try again in a few minutes, or import a track.".to_owned(),
    (false, _) => "Lyria returned no audio (the prompt or lyrics may have been blocked)".to_owned(),
  }
}

/// Whether a "completed" interaction carries no output and no error at all.
fn came_back_empty(interaction: &Value) -> bool {
  let no_steps = interaction.get("steps").and_then(Value::as_array).is_none_or(Vec::is_empty);
  no_steps && interaction.get("output_audio").is_none() && interaction_error(interaction).is_none()
}

/// A reply's field names and step types, for the log (never the audio itself).
fn reply_shape(interaction: &Value) -> String {
  let keys: Vec<&str> = interaction.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
  let steps: Vec<String> = interaction.get("steps").and_then(Value::as_array).into_iter().flatten().map(|s| format!("{}[{}]", s.get("type").and_then(Value::as_str).unwrap_or("?"), s.get("content").and_then(Value::as_array).into_iter().flatten().filter_map(|c| c.get("type").and_then(Value::as_str)).collect::<Vec<_>>().join(","))).collect();
  format!("fields {keys:?}, status {:?}, steps {steps:?}", interaction.get("status").and_then(Value::as_str))
}

/// The text that came with an interaction's output, blocks joined by blank lines.
fn interaction_text(interaction: &Value) -> String {
  output_blocks(interaction, "text").filter_map(|b| b.get("text").and_then(Value::as_str)).map(str::trim).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n")
}

enum VideoOut {
  Inline(String),
  Uri(String),
}

/// The first video in an interaction's model output (or the SDK-style `output_video`).
fn find_video(interaction: &Value) -> Option<VideoOut> {
  let from = |item: &Value| -> Option<VideoOut> {
    if let Some(data) = item.get("data").and_then(Value::as_str) {
      return Some(VideoOut::Inline(data.to_owned()));
    }
    item.get("uri").and_then(Value::as_str).map(|u| VideoOut::Uri(u.to_owned()))
  };
  let steps = interaction.get("steps").and_then(Value::as_array).into_iter().flatten();
  let in_steps = steps.filter(|s| s.get("type").and_then(Value::as_str).is_none_or(|t| t == "model_output")).flat_map(|s| s.get("content").and_then(Value::as_array).cloned().unwrap_or_default()).filter(|c| c.get("type").and_then(Value::as_str) == Some("video")).find_map(|c| from(&c));
  in_steps.or_else(|| interaction.get("output_video").and_then(from))
}

/// The tokens an interaction reports, for the spend meter.
fn omni_usage(interaction: &Value) -> Option<OmniUsage> {
  let usage = interaction.get("usage")?;
  let count = |v: Option<&Value>| v.and_then(Value::as_u64).unwrap_or(0);
  let by_modality: Vec<&Value> = usage.get("output_tokens_by_modality").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
  let video_tokens: u64 = by_modality.iter().filter(|m| m.get("modality").and_then(Value::as_str).is_some_and(|m| m.eq_ignore_ascii_case("video"))).map(|m| count(m.get("tokens"))).sum();
  let total_out = count(usage.get("total_output_tokens"));
  let input_tokens = count(usage.get("total_input_tokens"));
  (input_tokens > 0 || total_out > 0).then(|| {
    let video_tokens = if video_tokens == 0 { total_out } else { video_tokens };
    OmniUsage { input_tokens, video_tokens, other_output_tokens: total_out.saturating_sub(video_tokens) }
  })
}

/// Why an interaction failed, when it says.
fn interaction_error(interaction: &Value) -> Option<String> {
  let message = |e: &Value| e.get("message").and_then(Value::as_str).map(str::to_owned);
  interaction.get("error").and_then(message).or_else(|| interaction.get("errors").and_then(Value::as_array).and_then(|a| a.iter().find_map(message))).or_else(|| interaction.get("steps").and_then(Value::as_array).and_then(|s| s.iter().find_map(|s| s.get("error").and_then(message))))
}

/// The `response_format` field a "… cannot be set in response format" error names, when the body
/// still has it.
fn refused_format_field(err: &anyhow::Error, body: &Value) -> Option<&'static str> {
  let text = err.to_string().to_ascii_lowercase();
  if !text.contains("cannot be set in response format") {
    return None;
  }
  let format = body.get("response_format")?;
  [("aspect ratio", "aspect_ratio"), ("aspect_ratio", "aspect_ratio"), ("resolution", "resolution"), ("duration", "duration")].into_iter().find(|(words, key)| text.contains(words) && format.get(*key).is_some()).map(|(_, key)| key)
}

/// Whether Vertex said the model (or endpoint) doesn't exist.
fn is_not_found(err: &anyhow::Error) -> bool {
  let text = err.to_string();
  text.contains("error 404") || text.contains("NOT_FOUND")
}

/// Lyria 2's request: the count goes inside the instance (Google's samples), not in parameters.
fn lyria_body(prompt: &str, negative_prompt: Option<&str>, count: u16) -> Value {
  let mut instance = serde_json::Map::new();
  instance.insert("prompt".into(), json!(prompt));
  if let Some(negative) = negative_prompt.filter(|n| !n.trim().is_empty()) {
    instance.insert("negative_prompt".into(), json!(negative));
  }
  if count > 1 {
    instance.insert("sample_count".into(), json!(count));
  }
  json!({ "instances": [instance], "parameters": {} })
}

fn veo_body(request: &VeoRequest<'_>) -> Value {
  let frame = |image: &RefImage| json!({ "bytesBase64Encoded": BASE64.encode(&image.bytes), "mimeType": image.mime_type });
  let mut instance = serde_json::Map::new();
  instance.insert("prompt".into(), json!(request.prompt));
  if let Some(image) = request.start_frame {
    instance.insert("image".into(), frame(image));
  }
  if let Some(image) = request.end_frame {
    instance.insert("lastFrame".into(), frame(image));
  }
  if let Some(video) = request.extend {
    instance.insert("video".into(), json!({ "bytesBase64Encoded": BASE64.encode(video), "mimeType": "video/mp4" }));
  }
  let mut parameters = serde_json::Map::new();
  parameters.insert("sampleCount".into(), json!(request.sample_count.max(1)));
  if let Some(ratio) = request.aspect_ratio {
    parameters.insert("aspectRatio".into(), json!(ratio));
  }
  if let Some(resolution) = request.resolution {
    parameters.insert("resolution".into(), json!(resolution));
  }
  if let Some(seconds) = request.duration_seconds {
    parameters.insert("durationSeconds".into(), json!(seconds));
  }
  if let Some(audio) = request.generate_audio {
    parameters.insert("generateAudio".into(), json!(audio));
  }
  json!({ "instances": [instance], "parameters": parameters })
}

/// The first non-thought image in a `generateContent` reply.
fn first_image(response: &Value) -> anyhow::Result<GeneratedFile> {
  let candidates = response.get("candidates").and_then(Value::as_array).cloned().unwrap_or_default();
  for candidate in &candidates {
    for part in candidate.pointer("/content/parts").and_then(Value::as_array).into_iter().flatten() {
      if part.get("thought").and_then(Value::as_bool).unwrap_or(false) {
        continue;
      }
      if let Some(data) = part.pointer("/inlineData/data").and_then(Value::as_str) {
        let mime = part.pointer("/inlineData/mimeType").and_then(Value::as_str).unwrap_or("image/png");
        return Ok(GeneratedFile { bytes: BASE64.decode(data)?, extension: extension_for(mime), usage: None });
      }
    }
  }
  let reason = candidates.first().and_then(|c| c.get("finishReason")).and_then(Value::as_str).or_else(|| response.pointer("/promptFeedback/blockReason").and_then(Value::as_str));
  let text: String = candidates.iter().flat_map(|c| c.pointer("/content/parts").and_then(Value::as_array).cloned().unwrap_or_default()).filter_map(|p| p.get("text").and_then(Value::as_str).map(str::to_owned)).collect::<Vec<_>>().join(" ");
  match (reason, text.trim()) {
    (Some(reason), "") => anyhow::bail!("The model returned no image ({reason})"),
    (_, text) if !text.is_empty() => anyhow::bail!("The model returned no image: {text}"),
    _ => anyhow::bail!("The model returned no image"),
  }
}

/// The token counts in a reply's `usageMetadata`, for the spend meter.
fn usage(response: &Value) -> Option<GeminiUsage> {
  let meta = response.get("usageMetadata")?;
  let count = |key: &str| meta.get(key).and_then(Value::as_u64).unwrap_or(0);
  let image_tokens = meta.get("candidatesTokensDetails").and_then(Value::as_array).and_then(|details| details.iter().find(|d| d.get("modality").and_then(Value::as_str) == Some("IMAGE"))).and_then(|d| d.get("tokenCount")).and_then(Value::as_u64);
  Some(GeminiUsage { prompt_tokens: count("promptTokenCount"), candidates_tokens: count("candidatesTokenCount"), thoughts_tokens: count("thoughtsTokenCount"), image_tokens })
}

fn extension_for(mime: &str) -> &'static str {
  match mime {
    "image/jpeg" | "image/jpg" => "jpg",
    "image/webp" => "webp",
    _ => "png",
  }
}

/// The JSON body of a reply, or Google's error message.
async fn read_json(response: reqwest::Response) -> anyhow::Result<Value> {
  let status = response.status();
  let text = response.text().await?;
  if !status.is_success() {
    let message = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_owned)).unwrap_or_else(|| text.chars().take(300).collect());
    anyhow::bail!("Vertex AI error {status}: {message}");
  }
  Ok(serde_json::from_str(&text)?)
}

#[cfg(test)]
mod tests {
  use super::*;

  mod text_and_songs {
    use super::*;

    #[test]
    fn text_requests_put_the_track_before_the_prompt() {
      let body = text_body(&TextRequest { models: &TEXT_MODELS, system: "You mark up songs.", prompt: "Where are the sections?", audio: Some(("audio/aac", &[1, 2, 3])), json: true, temperature: 0.2 });
      assert_eq!(body["systemInstruction"]["parts"][0]["text"], "You mark up songs.");
      assert_eq!(body["contents"][0]["parts"][0]["inlineData"], json!({ "mimeType": "audio/aac", "data": "AQID" }));
      assert_eq!(body["contents"][0]["parts"][1]["text"], "Where are the sections?");
      assert_eq!(body["generationConfig"]["responseMimeType"], "application/json");
      let plain = text_body(&TextRequest { models: &TEXT_MODELS, system: "s", prompt: "p", audio: None, json: false, temperature: 1.0 });
      assert!(plain["generationConfig"].get("responseMimeType").is_none());
      assert_eq!(plain["contents"][0]["parts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn replies_leave_out_the_models_thoughts() {
      let reply = json!({ "candidates": [{ "content": { "parts": [{ "text": "hmm", "thought": true }, { "text": "{\"a\":" }, { "text": "1}" }] } }] });
      assert_eq!(reply_text(&reply).unwrap(), "{\"a\":1}");
      let blocked = reply_text(&json!({ "candidates": [{ "finishReason": "SAFETY" }] }));
      assert!(blocked.unwrap_err().to_string().contains("SAFETY"));
    }

    #[test]
    fn a_bad_request_about_the_model_isnt_an_answer() {
      assert!(rejects_the_model("Model lyria-3.5 is not supported for interactions."));
      assert!(rejects_the_model("Publisher Model `publishers/google/models/x` is not found."));
      assert!(!rejects_the_model("Invalid JSON payload: `instances` must not be empty."));
      assert!(!rejects_the_model("Required field `input` is missing."));
    }

    #[test]
    fn songs_run_in_the_background_and_stored() {
      assert_eq!(song_body("lyria-3.5", "a song", true), json!({ "model": "lyria-3.5", "input": "a song", "background": true, "store": true }));
      assert_eq!(song_body("lyria-3.5", "a song", false), json!({ "model": "lyria-3.5", "input": "a song" }));
      assert!(refuses_background(&anyhow::anyhow!("Vertex AI error 400 Bad Request: background is not supported for this model")));
      assert!(!refuses_background(&anyhow::anyhow!("Vertex AI error 403 Forbidden: permission denied")));
    }

    #[test]
    fn a_song_is_the_last_audio_block_with_its_text() {
      let finished = json!({ "status": "completed", "steps": [
        { "type": "model_output", "content": [{ "type": "text", "text": "[Verse]\nla" }, { "type": "audio", "mime_type": "audio/mpeg", "data": "AQID" }] },
        { "type": "model_output", "content": [{ "type": "text", "text": " structure " }, { "type": "audio", "data": BASE64.encode(b"RIFFxxxx") }] },
      ] });
      let file = song_audio(&finished, Route::Vertex).unwrap();
      assert_eq!((file.extension, file.bytes.as_slice()), ("wav", b"RIFFxxxx".as_slice()));
      assert_eq!(interaction_text(&finished), "[Verse]\nla\n\nstructure");
    }

    #[test]
    fn a_song_without_audio_says_whose_fault_it_is() {
      let bare = json!({ "status": "completed", "created": "2026-11-05T07:22:31Z", "object": "interaction", "model": "lyria-3-pro-preview" });
      assert!(came_back_empty(&bare));
      let on_vertex = song_audio(&bare, Route::Vertex).unwrap_err().to_string();
      assert!(on_vertex.contains("not your lyrics") && on_vertex.contains("Gemini API key"), "{on_vertex}");
      assert!(!song_audio(&bare, Route::GeminiApi).unwrap_err().to_string().contains("Gemini API key"));
      let declined = json!({ "status": "completed", "steps": [{ "type": "model_output", "content": [{ "type": "text", "text": "I can't make that song." }] }] });
      assert!(!came_back_empty(&declined));
      assert!(song_audio(&declined, Route::Vertex).unwrap_err().to_string().contains("It said: I can't make that song."));
      let failed = json!({ "status": "completed", "steps": [{ "type": "model_output", "error": { "message": "blocked by safety filters" } }] });
      assert_eq!(song_audio(&failed, Route::Vertex).unwrap_err().to_string(), "Lyria returned no audio: blocked by safety filters");
      assert_eq!(reply_shape(&declined), "fields [\"status\", \"steps\"], status Some(\"completed\"), steps [\"model_output[text]\"]");
    }
  }

  #[test]
  fn hosts_follow_the_location() {
    assert_eq!(host("global"), "https://aiplatform.googleapis.com/");
    assert_eq!(host("us"), "https://aiplatform.us.rep.googleapis.com/");
    assert_eq!(host("us-central1"), "https://us-central1-aiplatform.googleapis.com/");
  }

  #[test]
  fn image_requests_put_references_before_the_prompt() {
    let refs = [RefImage { mime_type: "image/png".into(), bytes: vec![1, 2, 3] }];
    let body = gemini_image_body(&GeminiImageRequest { model: "gemini-3-pro-image", prompt: "a fox", images: &refs, aspect_ratio: Some("16:9"), image_size: Some("2K") });
    assert_eq!(body["contents"][0]["parts"][0]["inlineData"]["data"], "AQID");
    assert_eq!(body["contents"][0]["parts"][1]["text"], "a fox");
    assert_eq!(body["generationConfig"]["imageConfig"], json!({ "aspectRatio": "16:9", "imageSize": "2K" }));
    let old = gemini_image_body(&GeminiImageRequest { model: "gemini-2.5-flash-image", prompt: "x", images: &[], aspect_ratio: None, image_size: Some("2K") });
    assert_eq!(old["generationConfig"]["imageConfig"], json!({}), "2.5 has no size setting");
  }

  #[test]
  fn thought_images_are_skipped() {
    let response = json!({ "candidates": [{ "finishReason": "STOP", "content": { "parts": [
      { "thought": true, "inlineData": { "mimeType": "image/png", "data": "AAAA" } },
      { "text": "here you go" },
      { "inlineData": { "mimeType": "image/jpeg", "data": "AQID" } }
    ] } }] });
    let image = first_image(&response).unwrap();
    assert_eq!((image.bytes, image.extension), (vec![1, 2, 3], "jpg"));
    let blocked = json!({ "candidates": [{ "finishReason": "IMAGE_SAFETY", "content": { "parts": [] } }] });
    assert!(first_image(&blocked).unwrap_err().to_string().contains("IMAGE_SAFETY"));
  }

  #[test]
  fn lyria_requests_put_the_count_in_the_instance() {
    assert_eq!(lyria_body("calm piano", Some("drums"), 2), json!({ "instances": [{ "prompt": "calm piano", "negative_prompt": "drums", "sample_count": 2 }], "parameters": {} }));
    assert_eq!(lyria_body("x", Some(" "), 1), json!({ "instances": [{ "prompt": "x" }], "parameters": {} }));
  }

  #[test]
  fn usage_breaks_out_image_tokens() {
    let response = json!({ "usageMetadata": { "promptTokenCount": 12, "candidatesTokenCount": 1130, "thoughtsTokenCount": 200, "candidatesTokensDetails": [{ "modality": "TEXT", "tokenCount": 10 }, { "modality": "IMAGE", "tokenCount": 1120 }] } });
    assert_eq!(usage(&response), Some(GeminiUsage { prompt_tokens: 12, candidates_tokens: 1130, thoughts_tokens: 200, image_tokens: Some(1120) }));
    assert_eq!(usage(&json!({})), None);
  }

  #[test]
  fn omni_requests_send_the_clip_and_ask_for_an_edit() {
    let request = OmniEditRequest { models: &OMNI_MODELS, prompt: "remove the tail", clip: &[1, 2, 3], resolution: "720p", output_gcs_uri: None };
    let body = omni_body("gemini-omni-1.1-flash-preview", &request);
    assert_eq!(body["input"][0], json!({ "type": "video", "mime_type": "video/mp4", "data": "AQID" }));
    assert_eq!(body["input"][1]["text"], "remove the tail");
    assert_eq!(body["generation_config"]["video_config"]["task"], "edit");
    assert_eq!(body["response_format"]["delivery"], "inline");
    assert!(body["response_format"].get("aspect_ratio").is_none(), "edits keep the clip's shape");
    let refused = anyhow::anyhow!("Vertex AI error 400 Bad Request: Resolution cannot be set in response format for edit task.");
    assert_eq!(refused_format_field(&refused, &body), Some("resolution"));
    assert_eq!(refused_format_field(&anyhow::anyhow!("Vertex AI error 400: quota"), &body), None);
    assert_eq!((body["background"].clone(), body["store"].clone()), (json!(true), json!(true)), "Vertex polls only stored background interactions");
    let to_gcs = omni_body("m", &OmniEditRequest { output_gcs_uri: Some("gs://b/out/"), ..request });
    assert_eq!((to_gcs["response_format"]["delivery"].clone(), to_gcs["response_format"]["gcs_uri"].clone()), (json!("uri"), json!("gs://b/out/")));
  }

  #[test]
  fn omni_results_are_found_in_the_model_output() {
    let done = json!({ "id": "i1", "status": "completed",
      "steps": [{ "type": "user_input", "content": [{ "type": "video", "data": "AAAA" }] },
                { "type": "model_output", "content": [{ "type": "text", "text": "ok" }, { "type": "video", "mime_type": "video/mp4", "data": "AQID" }] }],
      "usage": { "total_input_tokens": 57920, "total_output_tokens": 58000, "output_tokens_by_modality": [{ "modality": "video", "tokens": 57920 }] } });
    assert!(matches!(find_video(&done), Some(VideoOut::Inline(d)) if d == "AQID"), "the user's own clip is skipped");
    assert_eq!(omni_usage(&done), Some(OmniUsage { input_tokens: 57920, video_tokens: 57920, other_output_tokens: 80 }));
    let by_uri = json!({ "steps": [{ "type": "model_output", "content": [{ "type": "video", "uri": "gs://b/o.mp4" }] }] });
    assert!(matches!(find_video(&by_uri), Some(VideoOut::Uri(u)) if u == "gs://b/o.mp4"));
    let failed = json!({ "status": "failed", "steps": [{ "type": "model_output", "error": { "code": 3, "message": "blocked by safety filters" } }] });
    assert_eq!(interaction_error(&failed).as_deref(), Some("blocked by safety filters"));
    assert!(find_video(&failed).is_none());
  }

  #[test]
  fn veo_requests_carry_frames_and_settings() {
    let frame = RefImage { mime_type: "image/png".into(), bytes: vec![1, 2, 3] };
    let body = veo_body(&VeoRequest { model: "veo-3.1-fast-generate-001", prompt: "waves", start_frame: Some(&frame), end_frame: None, aspect_ratio: Some("9:16"), resolution: Some("1080p"), duration_seconds: Some(6), sample_count: 2, generate_audio: Some(false), extend: None });
    assert_eq!(body["instances"][0]["image"], json!({ "bytesBase64Encoded": "AQID", "mimeType": "image/png" }));
    assert!(body["instances"][0].get("lastFrame").is_none());
    assert_eq!(body["parameters"], json!({ "sampleCount": 2, "aspectRatio": "9:16", "resolution": "1080p", "durationSeconds": 6, "generateAudio": false }));
  }
}
