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

| Stage            | What you get                                                      | Runs on                                          |
|------------------|-------------------------------------------------------------------|--------------------------------------------------|
| Idea             | Lyrics and a music direction from a description (or your own)     | Writer: Gemini (Vertex) or Claude (Bedrock)      |
| Lyrics           | The words and direction, edited by hand or rewritten from notes   | Writer                                           |
| Song             | A sung song or 30-second clip, or a track you import              | Vertex (Lyria 3)                                 |
| Music Analysis   | Tempo and beats; mood, sections and timed lyrics                  | Your Mac (FFmpeg); Vertex (Gemini listening)     |
| Visual Style     | A look statement, palette, lighting and the common style block    | Writer                                           |
| Creative Brief   | Logline, arc, editing strategy, motifs, performer, venue, props   | Writer                                           |
| Reference Images | The performer's sheet and head shot, the venue and each prop      | Vertex (Nano Banana 2, or Pro in the final pass) |
| Scenes           | The song cut into scenes on its beats, each with an opening frame | Your Mac (timing); writer; Vertex (stills)       |
| Shots            | A silent clip per scene from its still; sung scenes lip-synced    | Vertex (Veo 3.1); fal (lip sync)                 |
| Final Video      | Every clip trimmed to its scene and laid under the song           | Your Mac (FFmpeg)                                |

- **What the writer makes is yours to edit.** Every part of the Visual Style and the Creative Brief
  is edited in place (props can be added and removed), and the reference images follow the brief's
  names and wording without losing their pictures. Each reference image can be drawn from its
  description, replaced with a picture of your own, or changed by a written instruction that keeps
  the rest of the picture. Edits apply to what is made after them: scenes already written keep
  their wording until they are planned again.
- **One section at a time, with the writer.** Each section of the Visual Style and the Creative
  Brief has a "Work on this with AI" link. It opens a conversation about that section alone: say
  what to change, read the writer's version (and edit it yourself), ask again, and put it back in
  the box when it is right. The writer sees the rest of the film for context but returns only that
  section, and nothing in the video changes until you put it back. Each reference image's
  description has the same link, and each card has a save button beside its upload button, so a
  picture can be saved, edited in another app and uploaded back to the same card.
- **Black and white means black and white.** When the look says so (black and white, monochrome,
  greyscale), every prompt says it outright and any reference image or still that comes back with
  colour has the colour taken out before it is saved. Clips are asked for the same; the Mono grade
  on the final cut covers any that slip.
- **Storyboard handling.** Double-click a still or reference image to see it full size. Cmd-click
  or Shift-click stills to select several, then redraw them together from the panel. In the draft
  pass, "The better model (Nano Banana Pro)" redraws the picked stills with the final pass's
  model, which holds a likeness better, while they stay draft stills at draft size.
- **Clips and lip sync side by side.** Veo makes the clips and fal syncs the mouths, so a lip sync
  can be started while clips are being made, and clips while a lip sync runs. Each leaves alone
  the scenes the other is working on. A clip run films the sung scenes first, since lip sync
  needs their clips; the lip-sync button opens when the last of them is in and can then run while
  Veo makes the rest. The clip and the vocal are put in fal's storage first,
  because fal's models read files from links only.
- **The words run under the pictures.** Each scene's card shows the line sung over it, so read
  across the grid the song's words run under the stills and clips like a track; the sung line is
  brighter on scenes the performer sings on camera. A scene can be marked as sung on the Shots
  stage as well as on Scenes, which turns the video's lip sync on if it was off.
- **A head shot beside the sheet.** On the performer's sheet the face is small, so close-ups have
  little to go on. The Face reference is a head-and-shoulders portrait drawn from the sheet, so it
  is the same person, and it goes with every still along with the sheet, the location and up to
  two props. It stays through edits to the brief and goes only when a rewritten brief replaces the
  performer. It works the other way too: upload a head shot to the Face card and it leads. The
  sheet's three poses are then drawn of that person (the card's redraw button, or the bulk draw),
  with build and clothes from the description; drawing everything again leaves the head shot
  alone, and it stays even when a rewritten brief replaces the performer's description. With a
  drawn head shot, the person button on the sheet's card makes it lead. Uploading a sheet, or
  redrawing the head shot from the sheet, hands the lead back to the sheet.
- **Everything a video makes is in the library.** Every version of every asset goes in as it is
  made, draft and final alike: the song, the vocal stem, reference images, stills, clips,
  lip-synced clips and each cut, plus tracks and pictures you bring in. Each is titled with the
  video's name and what it is ("scene 12 still, final, v3"). Opening a video adds whatever in
  its folder isn't there yet, so videos made earlier catch up.
- **The film says what it was cut from.** A cut uses the pass's own clip for each scene and falls
  back to the other pass's, then to a still. The Final Video stage says how many scenes would
  fall back, which pass the film on screen was cut in, and when clips or lip syncs made since
  have put it out of date.
- **Lineage: what a change overtakes.** A scene's assets are made in order: still, clip, lip sync,
  film, and in the final pass the still is redrawn from the draft still. Each one records what it
  was made from. Redraw a still and its clip, lip sync and final still stay where they are but
  are marked out of date; the left rail counts what each stage has to make or make again, the
  cards are tagged, and each stage's main button takes the out-of-date ones along with the
  missing ones. Nothing is thrown away until its replacement is made.
- **Fades.** The film can fade in from black and out to black (half a second to three seconds
  each), set on the Final Video stage. A fade runs smoothly across scene cuts. The sound fades
  with the picture only if asked, so a song with its own intro and ending is left as it is.
  Changing the fades or the finish marks the film as out of date until it is cut again.
- **The film opens in Edit Video as its parts.** "Open in Edit Video" on the Final Video stage
  sends the film over as its scenes in order, each with its length, and the song under them, at
  the same size, finish and fades. Edit Video's Film tab lists them: scenes can be moved, removed
  or given another clip (a file, or one made on the Trim, Extend or Describe tab), the soundtrack
  can be swapped, and the film is cut again on your Mac with the same frame-exact cut. A scene
  keeps its length, so the scenes around it stay on their beats; moving or removing one shifts
  what follows against the song.
- **Earlier plans are kept, and one scene can be written again.** "Plan again" starts a new
  storyboard and keeps the one it replaces, stills, clips and cut included, under Earlier plans
  on the Scenes stage; "Go back to this plan" swaps it back and keeps the current one the same
  way (the newest ten are kept). To fix a single scene, open it and use "Write this scene again",
  with an optional note to the writer: it is rewritten in the company of its neighbours and
  nothing else changes; its pictures stay until redrawn. For a plan made before earlier plans
  were kept, "Bring back drawn pictures" gives each scene the newest still, ending, clip and lip
  sync of its number still in the folder.
- **Every scene keeps its versions.** Before each change to a scene, a new opening or ending
  frame, clip, lip sync or a rewrite by the writer, the scene as it stood is kept: its words and
  its pictures in both passes together. "Keep this version" does it by hand. The scene editor
  lists them newest first with a thumbnail; click one to read it, and "Go back to this version"
  brings back its words and pictures (the scene as it is now is kept first). The newest twenty
  are kept, and no picture a version points at is deleted.
- **Every scene has an ending frame.** Veo is given a scene's opening still and makes up the
  rest, so a push-in from far away arrives at a face it invented. The planner therefore writes
  each scene's last frame as well as its first, and scenes planned before it did are given theirs
  the next time the Scenes stage is opened (or with "Write the missing ending frames"). The
  ending is drawn from the reference images with the scene's opening still beside it (same
  place, light and outfit), and the clip is filmed from one frame to the other. It is part of
  the lineage: it waits for its opening still, a redrawn opening puts it out of date, the clip
  waits for both and goes out of date when either changes, and the final pass redraws it from
  the draft. On the storyboard the ending shows as an inset in the card's corner: double-click the inset
  to see it full size, the rest of the card for the opening. Drawing it is on for every scene; "Draw the ending frame and film to it" turns it
  off for one scene, which keeps its description and is filmed from the opening frame alone.
  An ending frame is a second still per scene, so the stills cost about twice as much. One run
  draws both: the openings first, then each ending matched to its new opening. The draw button
  counts and prices pictures, not scenes, and says how many are openings and how many endings.
- **A copy for Instagram.** "Export for Instagram" on the Final Video stage makes a smaller copy
  of the cut film for uploading from a phone, beside it as `cut_vN_instagram.mp4` and in the
  library: H.264 in two passes at a variable 6 Mbps (peaks to 7.5), AAC at 192 kbps, at most 1080
  pixels on the short side, ready to stream. A film whose own file is already below 12 Mbps is
  encoded at half its own rate, so the copy is always well under the size of the cut. It keeps the cut's shape,
  so choose Reel, Feed or Square under Format first for a tall or square one. A newer cut marks
  the copy out of date.
- **Sung scenes keep the mouth clear.** The lip sync fails on anything in front of the mouth, so
  for a sung scene the planner, the still prompts (opening and ending) and the clip prompt all
  say: face to the lens, mouth and chin fully visible, no microphone of any kind, no hands, hair,
  drink, smoke or props in the way, nothing crossing in front. Microphones belong in unsung
  scenes.
- **Lip sync timing.** The vocal stem is separated from a WAV copy of the track, and each
  scene's slice is cut by decoding from the start, so both stay on the track's own clock. The
  slice sent to fal is kept beside the synced clip as `clip_NNN_draft_voice_vN.wav`: if a sync
  lands at the wrong moment, play that file against the clip to tell whether the sound given to
  fal was right (then the model is at fault) or not (then Gesso is).
- **Flowing scenes.** The transition into a scene is a cut or a flow. A flowing scene carries on
  from the one before as one continuous take: it has no opening frame of its own and opens on
  that scene's ending frame, so its clip starts on the exact picture the last one ended on, and
  runs of flowing scenes make longer sequences than one clip allows. The planner suggests which
  scenes flow (about a third, in runs of two to four where the music holds one feeling); change
  any of them under "Transition into this scene", or select several and use "Flow together" or
  "Cut between". It is part of the lineage: a redrawn ending is a new opening for the scene that
  flows from it, which puts that scene's ending and clip out of date, and one still run draws a
  whole chain in order. A scene can only flow from one whose ending frame is switched on.
- **Song length.** 30 seconds, 1, 1½, 2 or 3 minutes, set on the Idea or Song stage. The lyric
  writer is told how many verses and choruses to write for it, and Lyria how long to run. The
  length is asked for, not enforced: Lyria lands near it, and what is written decides most of it.
  Thirty seconds uses Lyria's clip model.
- **Instagram shapes.** The film can be cut as a Reel (9:16), a feed post (4:5) or a square (1:1)
  as well as the shape it was filmed in. Veo films 16:9 or 9:16 only, so the other shapes are the
  middle of each scene; the Final Video stage says how much of the picture is kept. For Instagram,
  film tall (Visual Style, Frame, 9:16) and cut at 1080p.
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
- **Earlier takes of the song are kept.** A new take (or an imported track) sets the old one
  aside with everything timed to it: the analysis, scenes, stills, clips and cut. The Song stage
  lists earlier takes newest first: click one to listen, "Go back to this take" swaps it in with
  its storyboard as it was left, and the take in use is set aside the same way. Songs made before
  this are found in the video's folder and listed without scenes. "Show the audio file" opens the
  folder, where the song is a file of its own.
- **Earlier lyrics are kept.** A rewrite, a new song take or going back to an older version keeps
  the title, music direction and lyrics as they were; "Keep this version" does it by hand. The
  Lyrics stage lists them newest first: click one to read it, and "Go back to this version" swaps
  it in, keeping the present words as a version too. The newest 40 are kept.
- **The performer's references are studio pictures.** The sheet and the head shot are drawn on
  a plain, empty white background whatever the visual style says: the look applies only to the
  performer's styling and the colour treatment, never to the background, so lasers, smoke or
  scenery in the style don't end up behind the face and leak into every still drawn from it.
- **Directions in the lyrics aren't sung.** Notes written among the words
  (`[Intro: guitar loop, drums kick in]`, `Outro [repeat to fade]`, `(Short Intro: ...)`) are taken
  out before the song is made and sent to Lyria as arrangement notes; the lyrics go as plain
  section tags (`[Intro]`, `[Verse 1]`) and sung lines. The Lyrics box keeps what you typed. Lines
  in round brackets with no section name are still sung, as backing vocals.
- **A refused song is asked for again.** Google's filter sometimes refuses a prompt it accepts a
  moment later ("Input blocked"). Gesso resubmits once, then without the directions found in the
  lyrics, then with the lyrics as typed, before reporting the error. Refusals aren't billed.
- **One shot.** "One shot" on the front page starts a new video and takes it all the way without
  a button pressed: a scout searches the web (Gemini with Google Search) for something trending
  this week and turns it into a song idea, then the song is written and made, the music analysed,
  the look designed, the brief written, the references and head shot drawn, the scenes planned,
  the stills and clips made, the sung scenes lip-synced (when fal is connected) and the film cut.
  The front page's One shot card settles what no one will be there to choose: what it should be
  about, lip sync, song length, pace, picture shape, the cut's format and the quality. While it
  works, a band across the top shows a bar of its thirteen steps, the step in hand and its
  progress, the page follows the stage being worked on, and the stages are dimmed and take no
  clicks. At a snag (a failed stage, or one needed twice in a row) the band turns red with "Needs
  your input", the stages come back to put it right, and "Continue one shot" carries on;
  "Continue one shot" in the rail also takes any video the rest of the way.
- **The page opens on your videos.** Music Video starts with the list of every saved video, with
  its stage, date, scene count and cost; click one to open it, or start a new one. "All videos"
  at the top of the rail goes back to the list, saving the open video as it stands.
- **Cost per video.** The left rail shows what the open video has cost so far, split into
  writing and planning, the song, reference images, scene images, clips and lip sync; the list on
  the Idea stage shows each saved video's cost. The all-time total for all videos, with a row per
  video, is on System Health. Every call is counted towards its video as it returns, at list
  prices (see `spend.json`). A video started before this was counted is priced once from the
  files in its folder, every version included, and that part is marked as an estimate: it leaves
  out the writer's calls and anything that failed or was deleted, and prices clips at the default
  model for their pass.
- **Errors name the account.** When a provider refuses for money or quota, the error opens by
  naming the account ("Gemini API key: out of credit or over its spending limit", "Google Cloud
  project: rate limit, not money") so the right one is topped up, or left alone.
- **A song falls back to Google Cloud.** Songs go through the Gemini API key when one is set;
  if its prepaid credit is spent (402), the song is asked of Vertex AI on the Cloud project
  instead, and charged there.
- **The Gemini API key is an account of its own.** Songs made through the key, and pictures it
  makes when Vertex has no capacity, are charged to "Gemini API key" on System Health and in the
  top bar's spend list, apart from Google Cloud, so the key's prepaid balance can be watched.
  The price per call is the same on both.
- **Draft, then final.** The Quality switch in the left rail picks the pass. **Draft** uses the
  cheap, fast models (Nano Banana 2 at 1K, Veo 3.1 Lite) to settle the direction. **Final** runs
  the same scenes again with the best ones: each still is redrawn by Nano Banana Pro at 2K with
  its approved draft still attached as the frame to recreate, then filmed by the Veo tier you
  pick (Veo 3.1 by default). The scenes, their wording and their timing are shared; each pass
  keeps its own stills and clips, and the final cut uses a draft clip wherever a final one hasn't
  been made yet.
- **Lip sync.** Veo's clips are silent, and neither cloud hosts a model that syncs a mouth to a
  track you supply, so this runs on fal with your own key (Settings). Tick **The performer sings
  on camera** on the Scenes stage and the writer marks the scenes to sing, framed as close-ups
  with the mouth in view; untick it and scenes keep to movement and cutaways. On the Shots stage,
  each sung scene's clip is trimmed to its slot and sent with the vocal under it to Sync Lipsync
  v2 (`lipsync-2` in the draft pass at $3 a minute, `lipsync-2-pro` in the final at $5). It
  follows the vocal stem, not the full mix: import one on the Song stage if your music app
  exports stems, or Gesso separates it once with Demucs on fal (about $0.13 for three minutes).
  Sung and shouted vocals sync less reliably than speech, so expect to redo a few scenes.
- **Projects.** Each video is a folder in `~/Artcraft/local/songs/` with `project.json`, the song,
  reference images, stills, clips and cuts. The song, the reference images and the finished film
  also go into the library; the dozens of stills and clips don't.

The pipeline's order, for a new video: run every stage in Draft; cut it; fix scenes until the
direction is right; switch to Final; redraw the stills; make the clips; lip-sync; cut again.

Optional settings in `local_providers.json`: `song_text_model` (default: `gemini-3.8-flash`, then
`gemini-3.5-flash`), `song_music_model` (default: `lyria-3-pro-preview`, then `lyria-3.5`) and
`song_claude_model` (default: `global.anthropic.claude-sonnet-4-6`). Lyria 3 and the listening
call run on the global endpoint.

Songs go through Vertex unless `gemini_api_key` is set (a Gemini API key from Google AI Studio, or
`ARTCRAFT_GEMINI_API_KEY`). With the key, the song is made through the Gemini API, which has
`lyria-3.5` (tried first there, then `lyria-3-pro-preview`). Vertex can finish a Lyria 3 request
without sending any audio; Gesso asks once more, then says so and points here.

The same key is a second route for pictures. Vertex serves the Gemini image models from capacity
shared between projects, with no per-project quota to raise, and answers small projects with
`429 Resource has been exhausted` often. When that happens and the key is set, that picture is made
through the Gemini API instead, on the key's own limit and billing. Without the key, or if the
Gemini API is full too, the run slows down and waits. Everything else stays on Vertex.

## System Health

**System Health** (System) shows whether each model can be reached with your accounts, and what
each account has cost. It checks when you open it and again on **Check again**. A check generates
nothing, so it's free.

| Models                          | How they're checked                               | Best result |
|---------------------------------|---------------------------------------------------|-------------|
| Gemini (writer and Nano Banana) | `countTokens` on your project                     | Ready       |
| Veo, Lyria, Gemini Omni Flash   | Catalogue, then an empty request                  | Ready       |
| Stability and Claude (Bedrock)  | `GetFoundationModelAvailability` for your account | Ready       |
| FFmpeg, background removal      | Looked for on your Mac                            | Ready       |

**Ready** means your account reached the model with a free call. For Veo, Lyria and Omni Flash,
which have no free call of their own, Gesso sends an empty request to the endpoint it generates
with: the model turns it down unbilled, which shows your project got through. Quota limits still
only show on a real generation. **Listed** means the provider has the model but that test didn't
get a clear answer. Problems are **No access** (the provider refused: an API not enabled, a role missing, or
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
