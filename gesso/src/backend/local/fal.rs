//! fal, with the user's own key: lip sync (Sync Lipsync v2) and vocal separation (Demucs) for the
//! Music Video page. Neither Google Cloud nor AWS hosts a model that syncs a mouth to a track the
//! user supplies, so these two run on fal.
//!
//! Requests go through fal's queue (`queue.fal.run`): submit, poll the status URL, then read the
//! response URL. Files are sent inline as data URIs, which fal accepts for file inputs; a few
//! seconds of video and audio is a few megabytes.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use log::warn;
use serde_json::{Value, json};

const QUEUE_HOST: &str = "https://queue.fal.run/";
pub const LIP_SYNC_MODEL: &str = "fal-ai/sync-lipsync/v2";
pub const VOCALS_MODEL: &str = "fal-ai/demucs";
const SUBMIT_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_secs(4);
/// Jobs not done by then are given up on.
const GIVE_UP: Duration = Duration::from_secs(15 * 60);

/// fal, called with the user's key.
#[derive(Clone)]
pub struct FalClient {
  http: reqwest::Client,
  key: String,
  /// Replaces the queue host (a mock server for testing).
  base_url: Option<String>,
}

impl FalClient {
  pub fn new(http: reqwest::Client, key: String, base_url: Option<String>) -> Self {
    Self { http, key, base_url }
  }

  /// Redraws the mouth in `video` (MP4 bytes) to match `audio` (WAV bytes), returning the new
  /// MP4. `pro` uses the costlier model with finer facial detail.
  pub async fn lip_sync(&self, video: &[u8], audio: &[u8], pro: bool) -> anyhow::Result<Vec<u8>> {
    let result = self.run(LIP_SYNC_MODEL, &lip_sync_body(video, audio, pro)).await?;
    let url = result.pointer("/video/url").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("fal's lip sync returned no video"))?;
    self.download(url).await
  }

  /// The vocal stem of `audio` (bytes of `mime_type`), as an MP3.
  pub async fn vocals(&self, audio: &[u8], mime_type: &str) -> anyhow::Result<Vec<u8>> {
    let result = self.run(VOCALS_MODEL, &vocals_body(audio, mime_type)).await?;
    let url = result.pointer("/vocals/url").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("fal's vocal separation returned no vocals"))?;
    self.download(url).await
  }

  /// Submits to the queue, waits, and returns the model's result.
  async fn run(&self, model: &str, body: &Value) -> anyhow::Result<Value> {
    let submit_url = format!("{}{model}", self.base_url.as_deref().unwrap_or(QUEUE_HOST));
    let submitted = read_json(self.authorised(self.http.post(submit_url)).json(body).timeout(SUBMIT_TIMEOUT).send().await?).await?;
    let (status_url, response_url) = match (submitted.get("status_url").and_then(Value::as_str), submitted.get("response_url").and_then(Value::as_str)) {
      (Some(status), Some(response)) => (status.to_owned(), response.to_owned()),
      // Some endpoints answer at once.
      _ => return Ok(submitted),
    };
    let deadline = tokio::time::Instant::now() + GIVE_UP;
    loop {
      match read_json(self.authorised(self.http.get(&status_url)).timeout(SUBMIT_TIMEOUT).send().await?).await {
        Ok(status) => match status.get("status").and_then(Value::as_str).unwrap_or_default() {
          "COMPLETED" => break,
          "IN_QUEUE" | "IN_PROGRESS" => {},
          other => anyhow::bail!("fal reported {other}: {}", failure(&status).unwrap_or_else(|| "no reason given".to_owned())),
        },
        Err(err) if tokio::time::Instant::now() < deadline => warn!("Polling fal failed (will retry): {err:#}"),
        Err(err) => return Err(err),
      }
      anyhow::ensure!(tokio::time::Instant::now() < deadline, "fal didn't finish in time");
      tokio::time::sleep(POLL_INTERVAL).await;
    }
    read_json(self.authorised(self.http.get(&response_url)).timeout(SUBMIT_TIMEOUT).send().await?).await
  }

  /// A result file. fal's file links are public, so the key isn't sent with them.
  async fn download(&self, url: &str) -> anyhow::Result<Vec<u8>> {
    let response = self.http.get(url).timeout(SUBMIT_TIMEOUT).send().await?.error_for_status()?;
    Ok(response.bytes().await?.to_vec())
  }

  fn authorised(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request.header("Authorization", format!("Key {}", self.key))
  }
}

fn lip_sync_body(video: &[u8], audio: &[u8], pro: bool) -> Value {
  json!({
    "model": if pro { "lipsync-2-pro" } else { "lipsync-2" },
    "video_url": data_uri("video/mp4", video),
    "audio_url": data_uri("audio/wav", audio),
    // The clip and the slice are cut to the same length; if they differ by a frame, stop at the shorter.
    "sync_mode": "cut_off",
  })
}

fn vocals_body(audio: &[u8], mime_type: &str) -> Value {
  json!({ "audio_url": data_uri(mime_type, audio), "model": "htdemucs", "stems": ["vocals"], "output_format": "mp3" })
}

fn data_uri(mime_type: &str, bytes: &[u8]) -> String {
  format!("data:{mime_type};base64,{}", BASE64.encode(bytes))
}

/// Why a job failed, when fal says.
fn failure(status: &Value) -> Option<String> {
  let detail = status.get("error").or_else(|| status.get("detail"))?;
  Some(detail.as_str().map_or_else(|| detail.to_string(), str::to_owned))
}

async fn read_json(response: reqwest::Response) -> anyhow::Result<Value> {
  let status = response.status();
  let text = response.text().await?;
  if !status.is_success() {
    let detail = serde_json::from_str::<Value>(&text).ok().and_then(|v| failure(&v)).unwrap_or_else(|| text.chars().take(300).collect());
    anyhow::bail!("fal error {status}: {detail}");
  }
  Ok(serde_json::from_str(&text)?)
}

#[cfg(test)]
mod tests {
  use std::io::{BufRead, BufReader, Read, Write};
  use std::net::TcpListener;
  use std::sync::{Arc, Mutex};

  use super::*;

  mod bodies {
    use super::*;

    #[test]
    fn lip_sync_sends_both_files_inline() {
      let body = lip_sync_body(&[1, 2, 3], &[4, 5], true);
      assert_eq!(body, json!({ "model": "lipsync-2-pro", "video_url": "data:video/mp4;base64,AQID", "audio_url": "data:audio/wav;base64,BAU=", "sync_mode": "cut_off" }));
      assert_eq!(lip_sync_body(&[], &[], false)["model"], "lipsync-2");
    }

    #[test]
    fn only_the_vocal_stem_is_asked_for() {
      let body = vocals_body(&[1, 2, 3], "audio/mpeg");
      assert_eq!(body["stems"], json!(["vocals"]));
      assert_eq!(body["audio_url"], "data:audio/mpeg;base64,AQID");
    }
  }

  /// The whole queue round trip against a stand-in server: submit with the key, poll until done,
  /// read the result, download the file.
  #[tokio::test(flavor = "multi_thread")]
  async fn a_job_is_submitted_polled_and_downloaded() {
    let (base_url, requests) = fake_fal();
    let client = FalClient::new(reqwest::Client::builder().no_proxy().build().unwrap(), "test-key".into(), Some(base_url));
    let synced = client.lip_sync(&[1, 2, 3], &[4, 5], false).await.unwrap();
    assert_eq!(synced, b"synced-video");
    let requests = requests.lock().unwrap();
    let lines: Vec<&str> = requests.iter().map(|(line, _)| line.as_str()).collect();
    assert_eq!(lines, ["POST /fal-ai/sync-lipsync/v2 HTTP/1.1", "GET /status HTTP/1.1", "GET /status HTTP/1.1", "GET /result HTTP/1.1", "GET /file.mp4 HTTP/1.1"]);
    assert!(requests[..4].iter().all(|(_, headers)| headers.contains("authorization: key test-key")));
    assert!(!requests[4].1.contains("authorization"), "the public file link doesn't get the key");
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn a_refused_key_says_so() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
      let (mut stream, _) = listener.accept().unwrap();
      let mut reader = BufReader::new(stream.try_clone().unwrap());
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
      reader.read_exact(&mut vec![0; length]).unwrap();
      let reply = r#"{"detail":"Invalid Key Authorization header format"}"#;
      write!(stream, "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).unwrap();
    });
    let client = FalClient::new(reqwest::Client::builder().no_proxy().build().unwrap(), "bad".into(), Some(url));
    let err = client.vocals(&[1], "audio/mpeg").await.unwrap_err().to_string();
    assert!(err.contains("fal error 401") && err.contains("Invalid Key"), "{err}");
  }

  /// A stand-in for fal's queue: the submit names its status and result URLs, the first status
  /// is still in progress, the result points at a file. Records each request line and headers.
  fn fake_fal() -> (String, Arc<Mutex<Vec<(String, String)>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (record, root) = (seen.clone(), base.clone());
    std::thread::spawn(move || {
      let mut polls = 0;
      for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let (mut headers, mut length) = (String::new(), 0);
        loop {
          let mut line = String::new();
          reader.read_line(&mut line).unwrap();
          if line == "\r\n" {
            break;
          }
          if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap();
          }
          headers.push_str(&line.to_ascii_lowercase());
        }
        reader.read_exact(&mut vec![0; length]).unwrap();
        let reply = if request_line.starts_with("POST") {
          json!({ "request_id": "r1", "status_url": format!("{root}status"), "response_url": format!("{root}result") }).to_string()
        } else if request_line.contains("/status") {
          polls += 1;
          json!({ "status": if polls < 2 { "IN_PROGRESS" } else { "COMPLETED" } }).to_string()
        } else if request_line.contains("/result") {
          json!({ "video": { "url": format!("{root}file.mp4") } }).to_string()
        } else {
          "synced-video".to_owned()
        };
        record.lock().unwrap().push((request_line.trim().to_owned(), headers));
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).unwrap();
      }
    });
    (base, seen)
  }
}
