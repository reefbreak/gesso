//! fal, with the user's own key: lip sync (Sync Lipsync v2) and vocal separation (Demucs) for the
//! Music Video page. Neither Google Cloud nor AWS hosts a model that syncs a mouth to a track the
//! user supplies, so these two run on fal.
//!
//! Requests go through fal's queue (`queue.fal.run`): submit, poll the status URL, then read the
//! response URL. The models take their files as public links only (inline `data:` URIs are
//! refused), so each file is first put in fal's own storage: ask for an upload link, `PUT` the
//! bytes there, and pass the file's link to the model.

use std::time::Duration;

use log::warn;
use serde_json::{Value, json};

const QUEUE_HOST: &str = "https://queue.fal.run/";
/// fal's storage, where files are put before a model is given their link: the current host, then
/// the one older clients use.
const STORAGE_HOSTS: [&str; 2] = ["https://rest.fal.ai/", "https://rest.alpha.fal.ai/"];
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
    let (video, audio) = futures::try_join!(self.upload(video, "video/mp4", "clip.mp4"), self.upload(audio, "audio/wav", "vocal.wav"))?;
    let result = self.run(LIP_SYNC_MODEL, &lip_sync_body(&video, &audio, pro)).await?;
    let url = result.pointer("/video/url").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("fal's lip sync returned no video"))?;
    self.download(url).await
  }

  /// The vocal stem of `audio` (bytes of `mime_type`), as an MP3.
  pub async fn vocals(&self, audio: &[u8], mime_type: &str) -> anyhow::Result<Vec<u8>> {
    let extension = if mime_type.contains("wav") { "wav" } else if mime_type.contains("aac") || mime_type.contains("mp4") { "m4a" } else { "mp3" };
    let audio = self.upload(audio, mime_type, &format!("track.{extension}")).await?;
    let result = self.run(VOCALS_MODEL, &vocals_body(&audio)).await?;
    let url = result.pointer("/vocals/url").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("fal's vocal separation returned no vocals"))?;
    self.download(url).await
  }

  /// Puts a file in fal's storage and returns its link, which fal's models can read.
  async fn upload(&self, bytes: &[u8], mime_type: &str, file_name: &str) -> anyhow::Result<String> {
    let hosts: Vec<&str> = self.base_url.as_deref().map_or_else(|| STORAGE_HOSTS.to_vec(), |base| vec![base]);
    let mut first_error = None;
    for host in hosts {
      match self.upload_to(host, bytes, mime_type, file_name).await {
        Ok(link) => return Ok(link),
        Err(err) => {
          warn!("Couldn't put {file_name} in fal's storage at {host}: {err:#}");
          first_error.get_or_insert(err);
        },
      }
    }
    Err(first_error.unwrap_or_else(|| anyhow::anyhow!("fal has no storage host configured")).context("Couldn't upload the file to fal"))
  }

  async fn upload_to(&self, host: &str, bytes: &[u8], mime_type: &str, file_name: &str) -> anyhow::Result<String> {
    let url = format!("{host}storage/upload/initiate?storage_type=fal-cdn-v3");
    let ticket = read_json(self.authorised(self.http.post(url)).json(&json!({ "content_type": mime_type, "file_name": file_name })).timeout(SUBMIT_TIMEOUT).send().await?).await?;
    let (Some(upload_url), Some(file_url)) = (ticket.get("upload_url").and_then(Value::as_str), ticket.get("file_url").and_then(Value::as_str)) else {
      anyhow::bail!("fal's storage gave no upload link");
    };
    // The upload link is signed, so the key isn't sent with it.
    self.http.put(upload_url).header("Content-Type", mime_type).body(bytes.to_vec()).timeout(SUBMIT_TIMEOUT).send().await?.error_for_status()?;
    Ok(file_url.to_owned())
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

fn lip_sync_body(video_url: &str, audio_url: &str, pro: bool) -> Value {
  json!({
    "model": if pro { "lipsync-2-pro" } else { "lipsync-2" },
    "video_url": video_url,
    "audio_url": audio_url,
    // The clip and the slice are cut to the same length; if they differ by a frame, stop at the shorter.
    "sync_mode": "cut_off",
  })
}

fn vocals_body(audio_url: &str) -> Value {
  json!({ "audio_url": audio_url, "model": "htdemucs", "stems": ["vocals"], "output_format": "mp3" })
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
    fn lip_sync_sends_links_to_both_files() {
      let body = lip_sync_body("https://f/clip.mp4", "https://f/vocal.wav", true);
      assert_eq!(body, json!({ "model": "lipsync-2-pro", "video_url": "https://f/clip.mp4", "audio_url": "https://f/vocal.wav", "sync_mode": "cut_off" }));
      assert_eq!(lip_sync_body("v", "a", false)["model"], "lipsync-2");
    }

    #[test]
    fn only_the_vocal_stem_is_asked_for() {
      let body = vocals_body("https://f/track.mp3");
      assert_eq!(body["stems"], json!(["vocals"]));
      assert_eq!(body["audio_url"], "https://f/track.mp3");
    }
  }

  /// The whole round trip against a stand-in server: both files are put in storage, their links
  /// are submitted with the key, the job is polled until done, and the result is downloaded.
  #[tokio::test(flavor = "multi_thread")]
  async fn a_job_is_submitted_polled_and_downloaded() {
    let (base_url, requests) = fake_fal();
    let client = FalClient::new(reqwest::Client::builder().no_proxy().build().unwrap(), "test-key".into(), Some(base_url));
    let synced = client.lip_sync(&[1, 2, 3], &[4, 5], false).await.unwrap();
    assert_eq!(synced, b"synced-video");
    let requests = requests.lock().unwrap();
    let lines: Vec<&str> = requests.iter().map(|(line, _, _)| line.as_str()).collect();
    // The two uploads run side by side, so their four requests come in either order.
    let (uploads, job) = lines.split_at(4);
    assert_eq!(uploads.iter().filter(|line| **line == "POST /storage/upload/initiate?storage_type=fal-cdn-v3 HTTP/1.1").count(), 2);
    assert_eq!(uploads.iter().filter(|line| line.starts_with("PUT /put/")).count(), 2);
    assert_eq!(job, ["POST /fal-ai/sync-lipsync/v2 HTTP/1.1", "GET /status HTTP/1.1", "GET /status HTTP/1.1", "GET /result HTTP/1.1", "GET /file.mp4 HTTP/1.1"]);
    for (line, headers, _) in requests.iter() {
      let signed_or_public = line.starts_with("PUT") || line.contains("/file.mp4");
      assert_eq!(headers.contains("authorization: key test-key"), !signed_or_public, "{line}: the key goes to fal's API, not to signed or public links");
    }
    let submitted: Value = serde_json::from_str(&requests[4].2).unwrap();
    assert!(submitted["video_url"].as_str().unwrap().ends_with("/files/clip.mp4") && submitted["audio_url"].as_str().unwrap().ends_with("/files/vocal.wav"), "{submitted}");
    assert!(requests.iter().any(|(line, headers, body)| line.starts_with("PUT /put/clip.mp4") && headers.contains("content-type: video/mp4") && body.as_bytes() == [1, 2, 3]));
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
    let err = format!("{:#}", client.vocals(&[1], "audio/mpeg").await.unwrap_err());
    assert!(err.contains("fal error 401") && err.contains("Invalid Key"), "{err}");
  }

  /// A stand-in for fal: storage hands out an upload link and a file link, the submit names its
  /// status and result URLs, the first status is still in progress, the result points at a file.
  /// Records each request's line, headers and body.
  fn fake_fal() -> (String, Arc<Mutex<Vec<(String, String, String)>>>) {
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
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let body = String::from_utf8_lossy(&body).into_owned();
        let reply = if request_line.contains("/storage/upload/initiate") {
          let name = serde_json::from_str::<Value>(&body).unwrap()["file_name"].as_str().unwrap().to_owned();
          json!({ "upload_url": format!("{root}put/{name}"), "file_url": format!("{root}files/{name}") }).to_string()
        } else if request_line.starts_with("POST") {
          json!({ "request_id": "r1", "status_url": format!("{root}status"), "response_url": format!("{root}result") }).to_string()
        } else if request_line.contains("/status") {
          polls += 1;
          json!({ "status": if polls < 2 { "IN_PROGRESS" } else { "COMPLETED" } }).to_string()
        } else if request_line.contains("/result") {
          json!({ "video": { "url": format!("{root}file.mp4") } }).to_string()
        } else {
          "synced-video".to_owned()
        };
        record.lock().unwrap().push((request_line.trim().to_owned(), headers, body));
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).unwrap();
      }
    });
    (base, seen)
  }
}
