# Gesso

Gesso makes images, video and music with **your own** cloud accounts (Google Cloud Vertex AI and
AWS Bedrock, with TypeSafe's Jev choosing the model), and hands the results to the Craft apps.
No subscription and no credits: each call is billed to your accounts, and Gesso meters it as it
goes.

Gesso is based on [ArtCraft](https://github.com/storytold/artcraft) by the ArtCraft Team: it is a
modified version of ArtCraft's pure-Rust desktop app (an [egui](https://github.com/emilk/egui) UI;
no Tauri and no JavaScript). It is not made, endorsed or supported by the ArtCraft Team, and it
carries none of their name, wordmark or logos as its own. See `NOTICE`.

## Running

```bash
cargo run -p gesso --release
```

The library, settings and spend ledger live in `~/Artcraft/` (the folder name is kept so an
existing library carries over).

- **Remote mode.** Set `ARTCRAFT_REMOTE=1` to use an ArtCraft account instead of your own clouds.
  Gesso then reuses an existing ArtCraft login from the ArtCraft desktop app's cookie jar
  (`<cache dir>/ai.artcraft.app/.cookies`), keeps its own copy in
  `~/Artcraft/credentials/native_session.json`, or signs in with the browser (device code) or a
  username and password. `~/Artcraft/settings/env_configs.json` selects the API host.

## Local mode (your own cloud accounts)

Gesso runs in **local mode** by default: no ArtCraft sign-in, no credits, and no calls to
ArtCraft's servers. Generations bill your own Google Cloud and AWS accounts, and the library lives
on disk in `~/Artcraft/local/`. Set `ARTCRAFT_REMOTE=1` to get the stock ArtCraft account flow back.

Configure it in `~/Artcraft/settings/local_providers.json` (written on first launch), or with
environment variables, which win over the file:

```json
{
  "gcp_project": "my-project",
  "gcp_location": "global",
  "gcp_video_location": "us-central1",
  "aws_region": "us-west-2",
  "aws_profile": "default",
  "jev_api_key": "ts_...",
  "jev_min_confidence": 0.35
}
```

| Setting           | Environment variable                            | Notes                                                       |
|-------------------|-------------------------------------------------|-------------------------------------------------------------|
| `gcp_project`     | `ARTCRAFT_GCP_PROJECT` / `GOOGLE_CLOUD_PROJECT` | Vertex AI; auth via `gcloud auth application-default login` |
| `gcp_api_key`     | `ARTCRAFT_GCP_API_KEY`                          | Express-mode key instead of a project; images only          |
| `bedrock_enabled` | —                                               | Default: on when AWS credentials are found                  |
| `aws_region`      | `ARTCRAFT_AWS_REGION`                           | Stability models are in `us-west-2`                         |
| `aws_profile`     | `AWS_PROFILE`                                   | Any SDK credential source works (SSO, env, roles)           |
| `jev_api_key`     | `ARTCRAFT_JEV_API_KEY` / `TYPESAFE_API_KEY`     | Turns on Jev routing for the Auto models                    |

Models offered (only for the accounts that are set up):

| Model              | Provider | Upstream ID                         |
|--------------------|----------|-------------------------------------|
| Nano Banana 2      | Vertex   | `gemini-3.1-flash-image`            |
| Nano Banana Pro    | Vertex   | `gemini-3-pro-image`                |
| Nano Banana 2 Lite | Vertex   | `gemini-3.1-flash-lite-image`       |
| Stable Image Ultra | Bedrock  | `stability.stable-image-ultra-v1:1` |
| SD 3.5 Large       | Bedrock  | `stability.sd3-5-large-v1:0`        |
| Stable Image Core  | Bedrock  | `stability.stable-image-core-v1:1`  |
| Veo 3.1 Fast       | Vertex   | `veo-3.1-fast-generate-001`         |
| Veo 3.1            | Vertex   | `veo-3.1-generate-001`              |
| Veo 3.1 Lite       | Vertex   | `veo-3.1-lite-generate-001`         |

**Auto** (image and video) routes each request. Models that can't honor the settings (aspect
ratio, resolution, reference count, frames, duration) are ruled out first. Jev then picks among
the rest from the prompt and settings; below `jev_min_confidence`, or without a Jev key, the first
fitting model in the order above is used (Nano Banana Pro when there are more than three
references). A toast says which model ran and why.

**Spend meter.** The top bar shows today's spend on your accounts next to the profile icon; click
it for today and this month by account (Google Cloud, AWS Bedrock, Jev). The app meters each call
itself, because neither cloud reports spend in real time: Gemini by the tokens the reply reports,
Veo by seconds of video, Stability per image, Jev by input tokens, all at October 2026 list prices
(`src/backend/local/pricing.rs`). Override a rate in `local_providers.json` under `"prices"`, e.g.
`{ "stability.sd3-5-large-v1:0": 0.065 }`. The ledger is `~/Artcraft/local/spend.json`.

**Cost before you generate.** Beside the generate button, the prompt box shows what the request
would cost at list prices: one figure for a chosen model, or for Auto the range across the models
that fit (hover for each model's price and the rules' likely pick). It updates as you change the
model, size, count, duration or audio, and asks no API, so it costs nothing.

**Routing panel.** The lightbox's side column shows how each local generation was made: what was
requested, who decided (Jev with its confidence, the rules, or you), the model and API model that
ran, Jev's odds for every option against the confidence bar, which models were ruled out and why,
the cost and how long it took.

**Tools.** In local mode the Studio pages work too. Each one has a stage, an options panel with the
cost, and a strip of results that also land in the library; any local image or video in the lightbox
has "Open in" buttons for them.

| Page          | What it does                                                                 | Runs on                                            |
|---------------|------------------------------------------------------------------------------|----------------------------------------------------|
| Edit Image    | Describe a change (optional painted mask); inpaint, erase, replace, expand   | Vertex (Nano Banana); Bedrock (Stability)          |
| BG Remove     | Transparent cut-out                                                          | Your Mac (ISNet), or Bedrock                       |
| Frame Extract | Marked moments or one frame every N seconds                                  | Your Mac (FFmpeg)                                  |
| Edit Video    | Trim, join, extend about 7 s, or describe a change ("remove the dog's tail") | Your Mac (FFmpeg); Vertex (Veo, Gemini Omni Flash) |
| 3D Stage      | Block a shot with people, shapes and imported GLB/OBJ, then paint it         | Your Mac; Vertex for Generate                      |
| Audio         | 30-second instrumental clips                                                 | Vertex (Lyria 2)                                   |

**Describe** (Edit Video) sends the clip to Gemini Omni Flash on Vertex with your change and
"Keep everything else the same". Omni edits 10 seconds at a time, so longer clips (up to 60 s) are
cut into even parts of 10 s or less, edited three at a time, and joined; each part is redrawn, so
details can shift where parts meet. Results come back at 24 fps, at 720p (or 1080p or 360p), with
the clip's own sound put back unless you untick it. It costs about $0.11 a second at 720p
(Google's token rates: $1.50/M input, $17.50/M video output). Omni runs on the global endpoint;
Gesso tries the model id `gemini-omni-1.1-flash-preview`, then `gemini-omni-1.1-flash`
(set `"omni_model"` to pick one). If a result is too big to come back inline, set
`"omni_output_gcs_uri": "gs://your-bucket/omni/"` and Gesso downloads it from there.

## Music Video (idea to finished film)

**Music Video** (Studio) is one staged workflow: describe a song, and end with a music video cut to
its beat. Each stage opens once the one before it has what it needs, shows its cost before it
runs, and leaves everything it made editable before the next stage uses it.

| Stage            | What you get                                                      | Runs on                                      |
|------------------|-------------------------------------------------------------------|----------------------------------------------|
| Idea             | Lyrics and a music direction from a description (or your own)     | Writer: Gemini (Vertex) or Claude (Bedrock)  |
| Lyrics           | The words and direction, edited by hand or rewritten from notes   | Writer                                       |
| Song             | A sung song or 30-second clip, or a track you import              | Vertex (Lyria 3)                             |
| Music Analysis   | Tempo and beats; mood, sections and timed lyrics                  | Your Mac (FFmpeg); Vertex (Gemini listening) |
| Visual Style     | A look statement, palette, lighting and the common style block    | Writer                                       |
| Creative Brief   | Logline, arc, editing strategy, motifs, performer, venue, props   | Writer                                       |
| Reference Images | The performer's three-view sheet, the venue and each prop         | Vertex (Nano Banana 2 or Pro)                |
| Scenes           | The song cut into scenes on its beats, each with an opening frame | Your Mac (timing); writer; Vertex (stills)   |
| Shots            | A silent clip per scene, animated from its still                  | Vertex (Veo 3.1 Lite, Fast or standard)      |
| Final Video      | Every clip trimmed to its scene and laid under the song           | Your Mac (FFmpeg)                            |

- **Timing is arithmetic, not a model's guess.** The beat grid comes from the track itself (onset
  envelope, autocorrelation, comb search), and every cut lands on a beat, on bar lines where the
  tempo allows. Higher-energy sections cut faster; Pace (Relaxed, Steady, Fast) scales the lot. The
  final cut counts frames from the song's start, so the picture can't drift over a whole song.
- **Consistency comes from the reference images.** Every still is drawn with the performer's sheet,
  the venue and the props attached, plus the common style block. Edit a description and draw it
  again before spending on scenes.
- **Cost.** The writer's calls are under a cent each and a song is about $0.08. Stills are about
  $0.07 each. Clips are the expensive stage: Veo is billed by the second, each scene is generated
  at 4, 6 or 8 seconds (the shortest that covers it; 1080p is always 8), so a three-minute song at
  the Steady pace is roughly 60 to 90 clips. Make one scene's clip first to check the look. Stop
  halts a run before its next picture or clip.
- **Lyrics are written new.** The writer is told to carry over an idea's energy and structure, not
  the words of an existing song, and to describe music without artist or song names (Lyria refuses
  them).
- **Lip sync.** Veo's clips are silent and aren't synced to the vocal, so the writer steers scenes
  toward movement and cutaways, not close-ups of singing.
- **Projects.** Each video is a folder in `~/Artcraft/local/songs/` with `project.json`, the song,
  reference images, stills, clips and cuts. The song, the reference images and the finished film
  also go into the library; the dozens of stills and clips don't.

Optional settings in `local_providers.json`: `song_text_model` (default: `gemini-3.8-flash`, then
`gemini-3.5-flash`), `song_music_model` (default: `lyria-3.5`, then `lyria-3-pro-preview`) and
`song_claude_model` (default: `global.anthropic.claude-sonnet-4-6`). Lyria 3 and the listening
call run on the global endpoint.

## System Health

**System Health** (System) shows whether each model can be reached with your accounts, and what
each account has cost. It checks when you open it and again on **Check again**. A check generates
nothing, so it's free.

| Models                          | How they're checked                               | Best result |
|---------------------------------|---------------------------------------------------|-------------|
| Gemini (writer and Nano Banana) | `countTokens` on your project                     | Ready       |
| Veo, Lyria, Gemini Omni Flash   | Vertex's model catalogue                          | Listed      |
| Stability and Claude (Bedrock)  | `GetFoundationModelAvailability` for your account | Ready       |
| FFmpeg, background removal      | Looked for on your Mac                            | Ready       |

**Ready** is proven with a free call on your account. **Listed** means the provider has the model,
but no free call shows whether your project may use it, so access is confirmed the first time it
runs. Problems are **No access** (the provider refused: an API not enabled, a role missing, or
model access not granted in Bedrock), **Not found** (the model id isn't offered there: renamed,
retired, or the wrong region) and **Check failed** (offline, or credentials expired). Each row
says what to do. The Bedrock check needs the `bedrock:GetFoundationModelAvailability` permission.
Jev isn't tested, because a routing call is billed.

**Costs** come from Gesso's own ledger (`~/Artcraft/local/spend.json`): totals for today, this
month and all time for Google Cloud, AWS Bedrock and Jev, then each account's spend model by
model with the number of billed calls. They're list prices times what each call used; your cloud
bills are the final word.

FFmpeg (`brew install ffmpeg`) is needed for video previews, Frame Extract, Edit Video and Music
Video. 3D Object
and 3D World still say "coming soon": they need a 3D generation provider.

Not in local mode yet: characters, and Bedrock video (Amazon Nova
Reel was retired on 2026-09-30; Luma Ray2 needs an S3 bucket). The code is in
`src/backend/local/`.

## Craft apps (hand-offs)

In local mode the lightbox has a **Craft apps** row: send the item to another app in the
[craft-suite](https://github.com/reefbreak/craft-suite).

| App         | Takes               | What happens                                                   |
|-------------|---------------------|----------------------------------------------------------------|
| FilmCraft   | image, video, audio | imported into the open project (`file.import`)                 |
| EffectCraft | image, video, audio | imported into the open project (`file.import`)                 |
| PhotoCraft  | image               | opened as a document (`app.open`)                              |
| LightCraft  | image               | added to the catalog (`library.import`)                        |
| VectorCraft | image               | placed in the open document (`file.place`)                     |
| DesignCraft | image               | placed in the open layout (`file.place`)                       |
| DeckCraft   | image               | inserted on the current slide (`insert.picture`)               |
| WordCraft   | image               | inserted at the cursor in the open document (`insert.picture`) |
| GridCraft   | image               | inserted as a picture in the open workbook (`insert.picture`)  |
| PrintCraft  | image               | a PDF made from the image (`pdfcraft --create-images`)         |

If the app is running from an earlier hand-off, the file goes over its control channel
(127.0.0.1, ports 47861–47869). Otherwise Gesso starts it with the file and that port. DeckCraft,
WordCraft and GridCraft don't open images from their command line, so Gesso starts them empty,
waits for the port, and sends the picture. WordCraft's channel needs the key it writes to
`control-key.local-47869` in its preferences folder (`~/Library/Application Support/WordCraft`
on macOS); Gesso reads it from there.
PhotoCraft and VectorCraft are started with the library's media folder as their read root, and
PhotoCraft with a control token in `~/Artcraft/settings/photocraft_control.token`.

Gesso finds the apps in this order: `suite_apps` in `local_providers.json` (binary name →
path), the suite checkout (`suite_dir`, `CRAFT_SUITE_DIR`, or found from Gesso's own build:
`<suite>/<app>/target/{release,debug}/<binary>`), `/Applications/<App>.app`, then `PATH`.

Coming back: FilmCraft (File › Send to Gesso) and PhotoCraft (File › Export › Send to
Gesso) write into `~/Artcraft/inbox/`. Gesso checks it every 2 seconds, adds new files to
the library, and moves them to `inbox/imported/`. Anything dropped there by hand is imported
too. Hidden files and `.part` / `.tmp` files are skipped until they are renamed.

## Architecture

| Module                 | What it does                                                                                  |
|------------------------|-----------------------------------------------------------------------------------------------|
| `backend`              | Tokio runtime and every API call; results come back to the UI as `Event`s                     |
| `backend::wire`        | Lenient response shapes; new server enum values can't break decoding                          |
| `backend::session`     | Credentials: own file, Tauri cookie import, env config                                        |
| `backend::media_cache` | Thumbnails and previews: downloaded and decoded off-thread, evicted when unused               |
| `backend::audio`       | One clip at a time: reference previews, feed tracks (pause, seek, volume), waveforms          |
| `backend::providers`   | Midjourney and fal: accounts, submitting, and a worker that finishes jobs into the library    |
| `models`               | Model catalog from OmniGen listings, plus names, families and limits                          |
| `prompt_box`           | The shared prompt box: editor, reference deck, keyframes, pickers, model selector             |
| `pages`                | Create Image, Video and Audio, Home, Library, and the shared page shell                       |
| `feed`                 | Generation feed: running, failed and finished jobs, library paging, grid and list             |
| `overlays`             | Lightbox, library picker, sign-in, settings, provider setup and the Midjourney sign-in window |
| `shell`                | Custom title bar, sidebar flush to the window edge, content panel header                      |
| `theme`, `ui`          | Tokens, Lucide-style icons, widgets, toasts, window chrome (drag, resize, caption buttons)    |

How the backend works:

- **Generation.** It goes through OmniGen: `/v1/omni_gen/{models,cost,generate}/{image,video,audio}`.
  Model capabilities come from the server, so new models and options appear without an app update.
- **Typed client functions.** Model lists, uploads, credits, login and deletes use
  `artcraft_client`.
- **Lenient decoding.** Generate and cost requests are JSON maps, as in the Tauri app. Job polling
  (`/v1/jobs/session`), the audio model list, the library list, batches and prompts are decoded
  with lenient structs.
- **Identity headers.** Every request sends the desktop identity headers: `Origin` and
  `User-Agent`.
- **Other providers.** Images can also run on Midjourney (your own account) or fal (your own API
  key), through `artcraft_router` like the Tauri app. The accounts live in the same files
  (`~/Artcraft/credentials`), so connecting in either app works in both. Those jobs are tracked in
  `~/Artcraft/state/native_provider_jobs.json` and their results uploaded into your library, resuming
  after a restart.

## Prompt box parity

| Feature                                                                             | Status                                                             |
|-------------------------------------------------------------------------------------|--------------------------------------------------------------------|
| Auto-growing prompt (~4 lines), expand/collapse, resize grip                        | Done                                                               |
| Focus mode (fullscreen editor with deck and toolbar)                                | Done                                                               |
| Length counter, red over the model's limit                                          | Done                                                               |
| Enter to generate (setting; Shift+Enter for a new line)                             | Done                                                               |
| `@Image1` / `@Video1` / `@Audio1` highlight and autocomplete                        | Done                                                               |
| Reference deck: fan, +N badge, hover-expand, reorder, clear all                     | Done                                                               |
| Keyframe slots: first/last frame, tilt, swap                                        | Done                                                               |
| Upload, pick from library, drag & drop, paste image                                 | Done (video uploads must be MP4)                                   |
| Model selector with families, logos, capability badges                              | Done                                                               |
| Aspect ratio, resolution, quality, count                                            | Done                                                               |
| Bitrate, output format, duration slider, sound, input mode                          | Done                                                               |
| Live credit cost on the generate button                                             | Done                                                               |
| Validation: starting frame, text-only banner, limits, uploads                       | Done                                                               |
| Clear all (confirms when references are attached)                                   | Done                                                               |
| `@Character` mentions and the Characters modal (create/edit/delete)                 | Done                                                               |
| Provider choice (Midjourney direct, fal), "Select Provider"                         | Done (images; video runs on ArtCraft)                              |
| In-app video playback                                                               | Done with FFmpeg on the PATH; otherwise opens in the system player |
| Audio reference playback (click to play/stop)                                       | Done                                                               |
| Animated video previews in the feed (with a stills toggle)                          | Done                                                               |
| Audio: style, instrumental, keep lyrics, loop                                       | Done                                                               |
| Audio: Beat & Key (BPM, musical key) and Tuning (sample rate, speed, volume, pitch) | Done                                                               |
| Audio references (play, 600 s cap) or one image, never both                         | Done                                                               |

Feed and lightbox:

- **Feed.** Pending cards show progress, time left and a batch banner. Failed cards show the reason
  and can be dismissed. The grid is masonry, with a list view alongside. Hover actions are Recreate,
  Make Video, Share and Download. Select mode does batch downloads. The last viewed item is marked.
- **Audio.** Audio cards play in place: a waveform (click to seek), the time and a volume flyout.
  One track plays at a time and stops when you leave the page. Audio offers Share and Download.
- **Lightbox.** Prev/next with the arrow keys, prompt copy and details. Actions are Recreate, Make
  Video, Download, Share and Delete. Audio plays in a panel with the full-size player.

## Diagnostics

Set `ARTCRAFT_SCREENSHOT=<file.png>` to save a screenshot once the window has settled.
`ARTCRAFT_PAGE` (`home`, `create-image`, `create-video`, `create-audio`, `music-video`, `health` or
`library`)
picks the page, `ARTCRAFT_MUSIC_VIDEO=<project id>` opens that saved Music Video project,
`ARTCRAFT_OPEN` (`settings` or `midjourney-login`) opens a dialog, and `ARTCRAFT_SCREENSHOT_EXIT=1`
closes the app afterwards. The screenshot shows egui only, not the sign-in webview.

## Tests

```bash
cargo test -p gesso
```

There are unit tests for the catalog, feed store, wire decoding and backend helpers. Headless UI
tests (`egui_kittest`) drive the prompt box (generate gating, Enter to generate, the pickers, `@`
mentions, clear-all), the Characters dialog and the audio toolbar (toggles, Beat & Key, Tuning).

## Fonts, icons and marks

`assets/fonts` holds Inter, Archivo and Geist Mono, all under the SIL Open Font License; the
licences sit next to the files. `assets/brand` is Gesso's own icon and mark (original artwork,
MIT OR Apache-2.0). `assets/services` holds the AI providers' logos, which are their owners'
trademarks, used only to show which service a model runs on; see `NOTICE`.
