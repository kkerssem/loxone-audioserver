# Bang & Olufsen BeoNetRemote API (BeoLink / ASE)

B&O speakers on the BeoLink / ASE platform (Beoplay M3; the A6, M5 and A9 2nd generation use the same API) are controlled over HTTP on **port 8080**, the "BeoNetRemote" API. It is unofficial; our checks were on a **Beoplay M3**, software 6.5, in November 2026.

Implementation: [`src/speaker/beo.rs`](../../src/speaker/beo.rs). Fixtures: [`tests/fixtures/speakers/beoplay_m3_notify_*.jsonl`](../../tests/fixtures/speakers/) (raw notification feeds).

## Discovery

mDNS service `_beocore._tcp`. TXT `name` (the speaker name) and `jid`, the stable id: `<type>.<item>.<serial>@products.bang-olufsen.com`. The control API is on port 8080 of the same host. Implementation: [`src/speaker/discovery.rs`](../../src/speaker/discovery.rs).

## Endpoints we use

| Endpoint | What it does | Status |
|----------|--------------|--------|
| `GET /BeoNotify/Notifications` | Long-poll feed of state changes (below) | Verified |
| `POST /BeoZone/Zone/Stream/Play`, then `POST …/Play/Release` | Play. A press is two requests: press, then release | Verified |
| `POST /BeoZone/Zone/Stream/Pause`, then `…/Pause/Release` | Pause. The speaker's own radio (`beoradio`) really pauses and resumes; on a Cast stream pause ends the stream | Verified |
| `PUT /BeoZone/Zone/Sound/Volume/Speaker/Level` `{"level":n}` | Volume, in the speaker's range (0–90 on the M3, from the feed). **Ignored while paused**: answers 200 and keeps its level | Verified |
| `PUT /BeoZone/Zone/Sound/Volume/Speaker/Muted` `{"muted":false}` | Unmute | Used, not checked separately |
| `GET /BeoDevice/powerManagement/standby` | `{"standby":{"powerState":"on"\|"standby"}}` | Verified |
| `PUT /BeoDevice/powerManagement/standby` `{"standby":{"powerState":"on"\|"standby"}}` | Wake / standby. The API stays up in standby; waking resets the volume to the speaker's default level | Verified |
| `POST /BeoZone/Zone/ActiveSources` `{"primaryExperience":{"source":{"id":"<source id>"}}}` | Select a source, e.g. `beoradio:<jid>` to bring the radio back after a voice message; then press Play | Used, not checked separately |

**Stalls:** the HTTP server sometimes takes several seconds to answer, and the request still takes effect. Send commands one at a time with a long timeout (we use 12 s per request); cancelling early would drop the Release half of a press.

## Notification feed

`GET /BeoNotify/Notifications` keeps the connection open and streams JSON objects separated by blank lines. It starts with a snapshot (current source, now playing, progress, volume) and then sends changes. The feed can go quiet without closing: we reconnect after 90 s of silence.

```json
{"notification":{"timestamp":"2026-11-15T11:38:19.186859","type":"NOW_PLAYING_NET_RADIO","kind":"playing","data":{…}}}
```

| `type` | `data` fields we read | Meaning |
|--------|-----------------------|---------|
| `SOURCE` | `primary` (source id), `primaryExperience.source.id`, `.friendlyName`, `.sourceType.type`, `primaryExperience.state` | Active source. An empty `data` (`{}`) means none: after standby |
| `SOURCE_EXPERIENCE_CHANGED` | same as `SOURCE` | Source change; one without a source id is ignored |
| `NOW_PLAYING_NET_RADIO` | `name` (station), `liveDescription` (current song), `image[]` (`url`, `size`), `stationId`, `genre`, `country` | Radio playing |
| `NOW_PLAYING_STORED_MUSIC` | `name`, `artist`, `album`, `duration` (s; 0 for a stream), `trackImage[]`, `albumImage[]`, `playQueueItemId`, `originalSource` | Track or stream, also Cast (`playQueueItemId: "googlecast"`) |
| `NOW_PLAYING_ENDED` | | Playback ended: stopped |
| `PROGRESS_INFORMATION` | `state` (`play`, `pause`, `stop`), `position`, `playQueueItemId` | Play state and position. `position` is in seconds, but larger than the track (so milliseconds) on some sources |
| `VOLUME` | `speaker.level`, `speaker.muted`, `speaker.range.minimum` / `.maximum` | Volume and its range |
| `SHUTDOWN` | `reason` (`standby`) | Going to standby |

The source's `_capabilities.supportedNotifications` also lists `PLAY_QUEUE_CHANGED` and `STREAMING_STATUS`, which we don't use.

The feed reports going to standby (`SHUTDOWN`) but not every wake. While the last state says standby, we ask `GET /BeoDevice/powerManagement/standby`.

## Source ids

A source id is `<source>:<jid>`. Seen: `beoradio` (B&O Radio, the built-in internet radio), `googlecast`. Our code also maps `radio`, `tunein` and `spotify`, which follow other people's notes.

Only `beoradio` can be selected again with `ActiveSources` and resumed. A Cast stream can only be started again by its Cast sender.
