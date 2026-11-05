//! AWS Bedrock, with the user's own account: the Stability image models through `InvokeModel`,
//! and Claude through `Converse` (the Music Video page's writer).
//!
//! Credentials come from the AWS SDK's usual chain (environment, `~/.aws` profiles and SSO,
//! instance roles) or a Bedrock API key in `AWS_BEARER_TOKEN_BEDROCK`. Amazon's Nova Canvas and
//! Nova Reel reached end of life on 2026-09-30, so they aren't called.

use std::sync::Arc;

use aws_sdk_bedrockruntime::Client;
use aws_sdk_bedrockruntime::primitives::Blob;
use aws_sdk_bedrockruntime::types::{ContentBlock, ConversationRole, InferenceConfiguration, Message, SystemContentBlock};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use super::GeneratedFile;
use super::RefImage;
use super::config::LocalConfig;

/// How strongly an image-to-image result departs from its reference (0 keeps it, 1 ignores it).
const IMAGE_TO_IMAGE_STRENGTH: f32 = 0.65;
/// Claude on Bedrock, through the global inference profile. `song_claude_model` overrides it.
pub const DEFAULT_CLAUDE_MODEL: &str = "global.anthropic.claude-sonnet-4-6";
/// A shot list for a whole song fits in this many tokens.
const CONVERSE_MAX_TOKENS: i32 = 16_000;

/// One Stability request (one image comes back per call).
pub struct StabilityRequest<'a> {
  pub model: &'a str,
  pub prompt: &'a str,
  /// `16:9` etc.; ignored with a reference image.
  pub aspect_ratio: Option<&'a str>,
  pub image: Option<&'a RefImage>,
}

/// A Claude reply and the tokens Bedrock billed for it.
#[derive(Debug)]
pub struct ConverseReply {
  pub text: String,
  pub input_tokens: u64,
  pub output_tokens: u64,
}

#[derive(Clone)]
pub struct BedrockClient {
  config: LocalConfig,
  client: Arc<OnceCell<Client>>,
}

impl BedrockClient {
  pub fn new(config: LocalConfig) -> Self {
    Self { config, client: Arc::new(OnceCell::new()) }
  }

  pub async fn generate_image(&self, request: StabilityRequest<'_>) -> anyhow::Result<GeneratedFile> {
    let body = stability_body(&request);
    let client = self.client().await;
    let output = client.invoke_model().model_id(request.model).content_type("application/json").accept("application/json").body(Blob::new(serde_json::to_vec(&body)?)).send().await.map_err(|err| anyhow::anyhow!("Bedrock error: {}", aws_sdk_bedrockruntime::error::DisplayErrorContext(err)))?;
    let response: Value = serde_json::from_slice(output.body.as_ref())?;
    first_image(&response)
  }

  /// One of Stability's image services (inpaint, erase, remove background, search and replace,
  /// outpaint). They're served only through the US cross-region profile, hence the `us.` prefix.
  pub async fn image_service(&self, model: &str, body: Value) -> anyhow::Result<GeneratedFile> {
    let client = self.client().await;
    let model_id = if model.starts_with("us.") { model.to_owned() } else { format!("us.{model}") };
    let output = client.invoke_model().model_id(model_id).content_type("application/json").accept("application/json").body(Blob::new(serde_json::to_vec(&body)?)).send().await.map_err(|err| anyhow::anyhow!("Bedrock error: {}", aws_sdk_bedrockruntime::error::DisplayErrorContext(err)))?;
    let response: Value = serde_json::from_slice(output.body.as_ref())?;
    first_image(&response)
  }

  /// Asks a text model (Claude) through the Converse API.
  pub async fn converse(&self, model: &str, system: &str, prompt: &str, temperature: f32) -> anyhow::Result<ConverseReply> {
    let client = self.client().await;
    let message = Message::builder().role(ConversationRole::User).content(ContentBlock::Text(prompt.to_owned())).build()?;
    let inference = InferenceConfiguration::builder().temperature(temperature).max_tokens(CONVERSE_MAX_TOKENS).build();
    let output = client.converse().model_id(model).system(SystemContentBlock::Text(system.to_owned())).messages(message).inference_config(inference).send().await.map_err(|err| anyhow::anyhow!("Bedrock error: {}", aws_sdk_bedrockruntime::error::DisplayErrorContext(err)))?;
    let text: String = output.output().and_then(|o| o.as_message().ok()).map(|m| m.content().iter().filter_map(|block| block.as_text().ok()).map(String::as_str).collect()).unwrap_or_default();
    anyhow::ensure!(!text.trim().is_empty(), "{model} returned no text ({})", output.stop_reason().as_str());
    let count = |n: i32| u64::try_from(n).unwrap_or(0);
    let (input_tokens, output_tokens) = output.usage().map_or((0, 0), |u| (count(u.input_tokens()), count(u.output_tokens())));
    Ok(ConverseReply { text, input_tokens, output_tokens })
  }

  async fn client(&self) -> &Client {
    self
      .client
      .get_or_init(|| async {
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest()).region(aws_config::Region::new(self.config.aws_region.clone()));
        if let Some(profile) = &self.config.aws_profile {
          loader = loader.profile_name(profile);
        }
        Client::new(&loader.load().await)
      })
      .await
  }
}

fn stability_body(request: &StabilityRequest<'_>) -> Value {
  let mut body = serde_json::Map::new();
  body.insert("prompt".into(), json!(request.prompt));
  body.insert("output_format".into(), json!("png"));
  match request.image {
    Some(image) => {
      body.insert("image".into(), json!(BASE64.encode(&image.bytes)));
      body.insert("strength".into(), json!(IMAGE_TO_IMAGE_STRENGTH));
      // Only SD 3.5 documents the mode switch; Ultra takes the image and strength alone.
      if request.model.starts_with("stability.sd3") {
        body.insert("mode".into(), json!("image-to-image"));
      }
    },
    None => {
      if let Some(ratio) = request.aspect_ratio {
        body.insert("aspect_ratio".into(), json!(ratio));
      }
    },
  }
  Value::Object(body)
}

fn first_image(response: &Value) -> anyhow::Result<GeneratedFile> {
  if let Some(data) = response.get("images").and_then(Value::as_array).and_then(|images| images.first()).and_then(Value::as_str) {
    return Ok(GeneratedFile { bytes: BASE64.decode(data)?, extension: "png", usage: None });
  }
  let reason = response.get("finish_reasons").and_then(Value::as_array).and_then(|r| r.iter().find_map(Value::as_str)).unwrap_or("no image returned");
  anyhow::bail!("Stability returned no image: {reason}")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn text_to_image_sends_the_ratio() {
    let body = stability_body(&StabilityRequest { model: "stability.stable-image-ultra-v1:1", prompt: "a fox", aspect_ratio: Some("16:9"), image: None });
    assert_eq!(body, json!({ "prompt": "a fox", "output_format": "png", "aspect_ratio": "16:9" }));
  }

  #[test]
  fn image_to_image_drops_the_ratio() {
    let image = RefImage { mime_type: "image/png".into(), bytes: vec![1, 2, 3] };
    let sd = stability_body(&StabilityRequest { model: "stability.sd3-5-large-v1:0", prompt: "x", aspect_ratio: Some("16:9"), image: Some(&image) });
    assert_eq!(sd["mode"], "image-to-image");
    assert_eq!(sd["image"], "AQID");
    assert!(sd.get("aspect_ratio").is_none());
    let ultra = stability_body(&StabilityRequest { model: "stability.stable-image-ultra-v1:1", prompt: "x", aspect_ratio: None, image: Some(&image) });
    assert!(ultra.get("mode").is_none());
  }

  #[test]
  fn filtered_results_say_why() {
    let image = first_image(&json!({ "seeds": [1], "finish_reasons": [null], "images": ["AQID"] })).unwrap();
    assert_eq!(image.bytes, vec![1, 2, 3]);
    let filtered = first_image(&json!({ "seeds": [1], "finish_reasons": ["Filter reason: prompt"] }));
    assert!(filtered.unwrap_err().to_string().contains("Filter reason: prompt"));
  }
}
