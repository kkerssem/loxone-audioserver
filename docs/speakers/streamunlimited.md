# StreamUnlimited speaker API (KEF W2, Naim Mu-so 2nd gen)

KEF's W2 platform (LS60 Wireless, LSX II, LS50 Wireless II) and Naim's Mu-so 2nd generation both run StreamUnlimited's "Stream800" streaming module (SUES). They share one local HTTP API on **port 80**. This page covers that API and the KEF-only paths. The Naim speakers also have their own API on port 15081: see [naim.md](naim.md).

The API is unofficial: it was reverse-engineered by others (reference: [pykefcontrol](https://github.com/N0ciple/pykefcontrol)) and by probing our own speakers. Unless a row says otherwise, behaviour was checked on an **LS60 Wireless** and a **Mu-so Qb 2** in November 2026.

Implementation: [`src/speaker/stream800.rs`](../../src/speaker/stream800.rs). Fixtures: [`tests/fixtures/speakers/`](../../tests/fixtures/speakers/) (`ls60_*`, `muso_player_data_*`).

**Confidence:** **Verified** = we sent it to a real speaker and saw the effect. **Read** = we read it from a real speaker but don't act on it. **Reported** = from other people's notes, not checked by us.

## Discovery

mDNS service `_sues800device._tcp`. TXT records:

| Key | Example | Use |
|-----|---------|-----|
| `manufacturer` | `KEF`, `Naim Audio` | Picks the vendor |
| `model` | `Mu-so Qb` | Display only |
| `name` | `Living room` | Speaker name as set in its app; matched against the Loxone zone name for automatic binding |
| `uuid` | `ls60w-<uuid>`, `naims810x-<uuid>` | Stable id: what `speaker_id` in `audioserver-config.json` holds |
| `ip` | | Address (the resolved host is used) |

Implementation: [`src/speaker/discovery.rs`](../../src/speaker/discovery.rs).

## Request forms

Every path is a typed node, like `player:volume` or `settings:/mediaPlayer/mute`.

| Verb | Form | Notes |
|------|------|-------|
| Read a value | `GET /api/getData?path=<path>&roles=value` | Answers a one-element array with the value, or `{"error":{"message":…}}` |
| Read a folder | `GET /api/getRows?path=<path>&roles=@all&from=<n>&to=<m>` | Answers `{"roles":{…}, "rows":[…], "rowsCount":n}`. **`to` is inclusive**: `from=0&to=3` returns four rows. Fails (HTTP 500) on leaf nodes and unknown roots |
| Write / act | `POST /api/setData` with body `{"path":…,"role":"value"\|"activate","value":…}` | `role: "value"` writes a setting; `role: "activate"` runs a control. A JSON body with `error` means it failed |

Typed values are objects with a type tag: `{"type":"i32_","i32_":42}`, `{"type":"bool_","bool_":true}`. Other tags seen: `i64_`, `double_`, `string_`, `kefPhysicalSource`.

## Nodes we use

| Path | Verb | Value | Status |
|------|------|-------|--------|
| `player:player/data` | getData | Current player: state, track, stream description, allowed controls (below) | Verified |
| `player:player/data/playTime` | getData | `{"type":"i64_","i64_":<ms>}`: position in the current track | Verified |
| `player:volume` | getData / setData `value` | `i32_` 0–100 | Verified |
| `settings:/mediaPlayer/mute` | getData / setData `value` | `bool_` | Verified |
| `player:player/control` | setData `activate` | Transport controls (below) | Verified |
| `settings:/kef/play/physicalSource` | getData / setData `value` | **KEF only.** `kefPhysicalSource`: the active input, which is also the power state (below) | Verified on LS60 |

## `player:player/data`

The fields we read (trimmed from [`muso_player_data_vtuner_radio_playing.json`](../../tests/fixtures/speakers/muso_player_data_vtuner_radio_playing.json)):

```json
{
  "state": "playing",
  "playId": { "systemMemberId": "<speaker uuid>", "timestamp": 545638249 },
  "trackRoles": {
    "title": "Radio Paradise - Main Mix",
    "icon": "https://…/2822.png",
    "type": "audio",
    "mediaData": { "metaData": { "serviceID": "VTuner", "live": true, "artist": "…", "album": "…" } }
  },
  "mediaRoles": {
    "type": "audio",
    "path": "vtuner:<base64>",
    "title": "Radio Paradise - Main Mix",
    "mediaData": { "metaData": { "serviceID": "VTuner", "live": true } }
  },
  "controls": { "next_": true, "previous": true, "seekTime": false, "seekTrack": true },
  "status": { "duration": -1 }
}
```

| Field | Meaning |
|-------|---------|
| `state` | `playing`, `paused`, `stopped`, or transitional (`transitioning`, `buffering`); re-read after ~300 ms until settled |
| `trackRoles` | What plays now: `title`, `icon` (cover URL), `mediaData.metaData` with `artist`, `album`, `serviceID`, `live`, `originalTrackNumber` |
| `mediaRoles` | The stream or item that was started. With a `path` it can be started again (see [Replaying a stream](#replaying-a-stream-or-playing-an-item)) |
| `mediaData.metaData.serviceID` | Where it comes from: `qobuz`, `spotify`, `airplay`, `googlecast`, `VTuner` (Naim radio), `airableRadios` (KEF radio), … |
| `mediaData.metaData.live` | `true` for internet radio: no pause, no seek |
| `controls` | Which `player:player/control` actions the current stream accepts. Missing or `false` means not allowed |
| `status.duration` | Track length in ms; `-1` or missing for live streams |
| `playId.timestamp` | Changes when a new stream starts: a cheap "is this still the same session" check |

States seen per source:

| Source | Playing | Stopped / paused |
|--------|---------|------------------|
| Qobuz Connect | Full track data, `controls.pause` and `seekTime` true | `paused` keeps the track |
| Speaker's own radio (vTuner, airable) | Full data, `live: true`, `controls.pause` false | After `stop`: `{"state":"stopped"}` with no track or `mediaRoles` at all |
| Google Cast | `serviceID: googlecast`, `mediaRoles.path` present | `paused` keeps the track |
| Standby (after waking) | | `stopped`, empty: there is no queue left to resume |

On a Naim, the radio station a stop cleared stays visible in [`/nowplaying`](naim.md#nowplaying) on port 15081.

## Transport: `player:player/control`

`POST /api/setData` with `{"path":"player:player/control","role":"activate","value":{"control":…}}`.

| `control` value | Effect | Status |
|-----------------|--------|--------|
| `{"control":"pause"}` | **Toggles** pause and resume. Sending it to a paused player resumes it | Verified (Mu-so, Qobuz Connect) |
| `{"control":"play"}` | Starts a **stopped** player. Does **not** resume a paused one. On an empty player (after standby, or radio after `stop`) it fails with "Directory is empty" | Verified |
| `{"control":"stop"}` | Stops. On live radio this clears the player data | Verified |
| `{"control":"seekTime","time":<ms>}` | Seek within the track; only when `controls.seekTime` is true | Verified (Mu-so, Qobuz Connect) |
| `{"control":"play","mediaRoles":<mediaRoles>}` | Starts that stream or item (see next section) | Verified |
| `{"control":"next_"}`, `{"control":"previous"}` | Answer 200 but **did nothing** on a Qobuz album started from `getRows`. On a Naim, use [`/nowplaying?cmd=next`](naim.md#nowplaying) | Verified (no effect, Mu-so) |
| `{"control":"seekTrack", …}` | Listed in `controls`, but `trackNumber`, `index` and `track` parameters did nothing; the right form is unknown | Tried, unknown |

Reported by others: port-80 `play`, `next_` and `previous` end a Spotify Connect session, so Naim transport is better sent to port 15081.

Live radio lists no `pause` control. A Naim ignores the toggle; a KEF LS60 answers `pause` with **HTTP 500**. So "pause" on radio is a `stop`, and "play" restarts it (next section).

## Replaying a stream or playing an item

`play` with a `mediaRoles` object starts that stream. The `mediaRoles` can come from:

- **A stopped live stream:** `player:player/data` `mediaRoles` read before the `stop`. Verified on an LS60 (airable radio) and a Mu-so (vTuner). This is how play restarts radio, how a radio station started from the speaker's app becomes a room favorite, and how radio comes back after a voice message.
- **A `getRows` row:** a row is itself a `mediaRoles` object (see [Browsing](#browsing-getrows)). Verified on the Mu-so for a Qobuz album and Qobuz tracks.

Not replayable:

- A **Google Cast** stream (`serviceID: googlecast`) has a `path`, but only its Cast sender can start it. Replaying is accepted and plays nothing, and `play` fails afterwards (LS60, 2026-11-15).
- **Qobuz Connect** from the Qobuz app has no `path`.

`mediaRoles.path` per source:

| Source | `path` |
|--------|--------|
| Naim radio (vTuner) | `vtuner:<base64>`: the base64 holds the station id, stream id and a `radioeu.naimaudio.com/api/ir/stationstream?id=…` URL |
| KEF radio (airable) | `airable:https://…airable.io/…` |
| Qobuz item (browsed) | `qobuz:/track?id=<id>`, `qobuz:/album?id=<id>`, `qobuz:/playlist?id=<id>` |
| Qobuz from the Naim app | `playlists:pq/getitems` (the Naim play queue) |
| Qobuz Connect (Qobuz app) | none |

When a Qobuz item plays, `trackRoles.mediaData.activeResource.uri` holds a **signed stream URL with the account id**. Never store or log it.

## Browsing: `getRows`

Folder trees of the services the speaker is logged into. Checked on the Mu-so (2026-11-18); KEF not checked.

| Root | Result |
|------|--------|
| `qobuz:` | `roles.title` "Qobuz root", with rows: Discover (`qobuz:/featured/`), My Playlists (`qobuz:/user/playlists/`), Favourites (`qobuz:/user/favourites/`), Purchases (`qobuz:/user/purchases/`), Search (`qobuz:/search/node`, `type: "query"`). `roles.search` describes `qobuz:searchContainer` |
| `qobuz:/featured/` | Albums (`featured/albums`, 100), Artists (100), Playlists (`featured/playlists`, 1024) |
| `qobuz:/artist?id=<id>` | The artist's albums |
| `qobuz:/user/purchases/` | Tracks, Albums |
| `qobuz:/search/{artists,albums,tracks,playlists}?query=<text>` | Search results of that kind (`rowsCount` capped at 1000). `qobuz:/search/node?query=…` returns artists only; without a `query` the speaker answers HTTP 500 "Missing parameter" |
| `qobuz:/user/favourites/` | Tracks (`containerType: "playlist"`, playable as a whole), Albums, Artists |
| `playlists:pq/getitems` | The Naim play queue (filled by Naim favourites and the Naim app) |
| `vtuner:` | Empty, with `rowsRedirect: "vtuner:loginPage/55"` |
| `ui:`, `playlists:`, `airable:` | HTTP 500 |

Row shape:

| Field | Meaning |
|-------|---------|
| `title`, `icon` | Display name and cover |
| `path` | `qobuz:/album?id=…`, `qobuz:/track?id=…`, … |
| `type` | `container` (open it with `getRows`) or `audio` (a track) |
| `containerType` | `none`, `playlist`, `search` |
| `containerPlayable` | `true` when the whole container can be played |
| `mediaData.metaData` | Artist, album, `serviceID` |

Row `path` forms: `qobuz:/album?id=…`, `qobuz:/playlist?id=…`, `qobuz:/track?id=…`, `qobuz:/artist?id=…` (`containerType: "person"`).

Playing (all via `play` + the row as `mediaRoles`, Mu-so 2026-11-18):

| Row | Result |
|-----|--------|
| Album | Plays from track 1; [`/nowplaying?cmd=next`/`prev`](naim.md#nowplaying) move through the album |
| Track inside an album | Plays that track only, as a one-track queue: `next` stays on it |
| Single track (e.g. favourite tracks) | Plays that track |

These plays do not show up in the Naim play queue (`playlists:pq/getitems`).

A minimal `mediaRoles` is enough for containers: `{"type":"container","path":"qobuz:/album?id=…","containerPlayable":true}` plays an album or playlist. A track needs more: `{"type":"audio","path":…}` alone stops the player; with `mediaData.metaData.prePlayPath` (`qobuz:/preplay?id=…`) and `serviceID: "qobuz"` it plays, but reports no title unless the `mediaRoles` carries `title` (Mu-so, 2026-11-18).

While a browsed item plays, `trackRoles.path` is the track (`qobuz:/track?id=…`) and `mediaRoles.path` the album or playlist that was started.

## KEF: input and power

`settings:/kef/play/physicalSource` (`kefPhysicalSource`) is the active input and the power state at once.

| Value | Meaning |
|-------|---------|
| `wifi` | Streaming (app, Qobuz, AirPlay, Cast, radio) |
| `optic`, `analog`, `tv`, `bluetooth`, … | Physical inputs. Names follow pykefcontrol; on our LS60 only `wifi`, `standby` and `powerOn` were checked |
| `standby` | Auto-standby |
| `powerOn` | Standby from the KEF app. The app shows standby while the source reads `powerOn` (LS60, verified) |

- **Standby:** write `standby`.
- **Wake:** select an input. Writing `powerOn` leaves an LS60 in its app-standby state; selecting an input (the last one seen, or `wifi`) wakes it.
- `powerOn` is also reported briefly while a KEF wakes up; it is not an input.

## Volume

`player:volume` (`i32_`, 0–100). A muted speaker keeps its level; the Loxone App shows mute as volume 0, so raising the volume also writes `mute` false. Speakers report a new level a moment late: a second quick step must not start from the old reading.
