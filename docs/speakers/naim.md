# Naim API (port 15081)

Naim's 2nd-generation Mu-so speakers run the [StreamUnlimited API](streamunlimited.md) on port 80, plus Naim's own REST API on **port 15081**. This page covers the 15081 API. It is unofficial; our checks were on a **Mu-so Qb 2** (API `version` 1.4.0, app build 4.5.0) in November 2026. Several rows come from a community write-up, ["The Naim Mu-so 2 HTTP bible"](https://community.naimaudio.com/t/the-naim-mu-so-2-http-bible/45357), and are marked **Reported**.

Implementation: [`src/speaker/stream800.rs`](../../src/speaker/stream800.rs) (`Vendor::Naim`). Fixture: [`muso_15081_power_on.json`](../../tests/fixtures/speakers/muso_15081_power_on.json).

## Conventions

- Plain HTTP on port 15081, JSON replies. Every object carries `version`, `name`, `ussi` (its path), `class` and `cpu`.
- **All values are strings**, numbers and booleans included: `"transportState": "2"`, `"live": "1"`.
- Commands are mostly `GET <path>?cmd=<command>`. Some settings use `PUT <path>?<key>=<value>`.
- `GET /` answers `{"ussi":"api/1","class":"object.api.support",…}`.
- A missing path answers HTTP 404.

## Endpoints

| Endpoint | What it does | Status |
|----------|--------------|--------|
| `GET /power` | `system`: `on`, or `lona` (network standby) / `standby` | Verified |
| `PUT /power?system=on` / `?system=lona` | Wake / standby. `GET` with these parameters does nothing | Verified |
| `GET /nowplaying` | What plays, also after a stop (below) | Verified |
| `GET /nowplaying?cmd=<command>` | Transport (below) | Verified for `play`, `next`, `prev` |
| `GET /levels/room` | `volume` (0–100) and `mute` | Reported |
| `PUT /levels/room?volume=<0-100>`, `?mute=0\|1` | Set volume / mute. There is no step command: `volume=+2` is read as 2, and `cmd=volup` is ignored | Reported |
| `GET /inputs` | All inputs with `name`, `ussi`, `disabled`, `selectable` | Verified |
| `GET /inputs/<name>?cmd=select` | Select an input: `ana`, `dig`, `hdmi`, `radio`, `spotify`, `playqueue`. `radio` restarts the last station. Passive inputs (`airplay`, `gcast`, `qobuz`, `tidal`, `bluetooth`, `upnp`, `usb`, `multiroom`, `files`) switch on by themselves when playback arrives | Reported |
| `GET /inputs/qobuz` | The Qobuz login: `username` (an email), `userId`, `quality`. **Personal data: never log or store** | Verified |
| `GET /inputs/radio` | Radio menus: Favourites, Added Stations, Popular, Genre, Brands, by country, … (`ussi` `inputs/radio/<base64>`) | Verified |
| `GET /favourites` | Naim favourites (below) | Verified |
| `GET /favourites/<id>?cmd=play` | Play a favourite (below) | Verified |
| `GET /system`, `/network`, `/update`, `/alarms` | Device info, read only. `/system` includes serials | Verified (`/system`), Reported |
| `/qobuz`, `/playqueue`, `/presets` | Do not exist (404) | Verified |

## `/nowplaying`

Only answers while the speaker is awake. Unlike port 80, it **keeps the stopped radio station**: after a `stop` it still reports the station, title and artwork, with `transportState` 1.

| Field | Meaning |
|-------|---------|
| `transportState` | `1` stopped, `2` playing, `3` paused |
| `source` | `inputs/radio`, `inputs/qobuz` (browsed Qobuz), `inputs/playqueue` (Naim app and favourites), … |
| `title`, `artistName`, `albumName`, `station`, `artwork` | What plays (or played) |
| `live` | `"1"` for internet radio |
| `canResume` | `"1"` when `cmd=play` can restart what stopped |
| `transportPosition` | Position in ms |
| `restrictPause`, `restrictSeek`, `restrictSkipNext`, `restrictSkipPrev`, `restrictResume`, `restrictStop`, … | `0` allowed, `128` not allowed for the current source (Reported). Live radio allows only stop and resume |

Commands:

| Command | Effect | Status |
|---------|--------|--------|
| `cmd=play` | Restarts what `/nowplaying` remembers (a stopped radio station). Only `GET` works: `PUT` answers 200 and does nothing | Verified |
| `cmd=next`, `cmd=prev` | Next / previous track. Works on a Qobuz album started from port-80 `getRows`, where port-80 `next_` does nothing | Verified (2026-11-18) |
| `cmd=playpause` | Toggle pause | Reported |
| `cmd=stop` | Stop | Reported |

## Favourites

`GET /favourites` lists the favourites saved in the Naim app (2,004 entries on our Mu-so, Qobuz albums and playlists as well as radio stations):

```json
{
  "name": "Some Album",
  "ussi": "favourites/<32-hex id>",
  "class": "object.favourite",
  "artist": "Some Artist",
  "artwork": "https://static.qobuz.com/images/covers/…_600.jpg",
  "favouriteClass": "object.qobuzAlbum",
  "favouriteUssi": "inputs/qobuz/albums/<qobuz album id>",
  "available": "1"
}
```

`favouriteClass` values seen: `object.qobuzAlbum`, `object.qobuzPlaylist`.

`GET /favourites/<id>?cmd=play` plays it straight away, waking the speaker from standby. A Qobuz album loads into the Naim play queue (`source` `inputs/playqueue`), which port 80 lists at [`getRows playlists:pq/getitems`](streamunlimited.md#browsing-getrows). Verified on the Mu-so, 2026-11-18.

## What we use it for

| Need | Endpoint |
|------|----------|
| Power state and wake / standby | `/power` |
| Restart radio stopped by pause | `/nowplaying?cmd=play` when `canResume` is `"1"`; otherwise port-80 `play` with the kept `mediaRoles` |
| Physical input name (HDMI, optical, …) | Not read yet: it would cost an extra request per poll, so a Mu-so on a physical input shows as Wi-Fi |
