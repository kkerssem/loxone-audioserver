# Speaker APIs

The local network APIs the audioserver uses to control speakers. They are all unofficial: what is written here comes from our own probes of real speakers and from other people's notes, and each page marks which is which. For the APIs the audioserver itself offers to Loxone, see [web-ui-protocol.md](../web-ui-protocol.md) (port 7091) and [miniserver-protocol.md](../miniserver-protocol.md) (port 7095).

| Speakers | API | Page | Backend |
|----------|-----|------|---------|
| KEF W2 (LS60 Wireless, LSX II, LS50 Wireless II), Naim Mu-so 2nd gen | StreamUnlimited, HTTP port 80 | [streamunlimited.md](streamunlimited.md) | `kef_stream800`, `naim_stream800` in [`stream800.rs`](../../src/speaker/stream800.rs) |
| Naim Mu-so 2nd gen | Naim REST, HTTP port 15081 | [naim.md](naim.md) | `naim_stream800` |
| Bang & Olufsen BeoLink / ASE (Beoplay M3, A6, M5, A9 2nd gen) | BeoNetRemote, HTTP port 8080 | [beo.md](beo.md) | `beo` in [`beo.rs`](../../src/speaker/beo.rs) |
| KEF first generation (LSX, LS50 Wireless) | Binary protocol, TCP port 50001 | [below](#kef-first-generation) | `kef_legacy`: discovered, not implemented |

## Comparison

| | KEF W2 | Naim Mu-so 2 | B&O ASE |
|--|--------|--------------|---------|
| Discovery (mDNS) | `_sues800device._tcp` | `_sues800device._tcp` | `_beocore._tcp` |
| Stable id | TXT `uuid` | TXT `uuid` | TXT `jid` |
| State | Poll `player:player/data` | Poll `player:player/data`, `/power` | Push: `/BeoNotify/Notifications` feed |
| Power | Input `standby` / select an input | `/power?system=lona\|on` | `powerManagement/standby` |
| Pause | `pause` control (a toggle) | `pause` control (a toggle) | `Stream/Pause` press + release |
| Live radio pause | None: stop, then replay `mediaRoles` | None: stop, then `/nowplaying?cmd=play` | Real pause (`beoradio`) |
| Next / previous | Untested | `/nowplaying?cmd=next\|prev` | Not used |
| Seek | `seekTime` control (checked on Naim only) | `seekTime` control | Not used |
| Volume | `player:volume` 0–100 | `player:volume` 0–100 | `Level` 0–90, ignored while paused |
| Browse a music service | `getRows` (not checked on KEF) | `getRows` `qobuz:` | Not known |
| Quirks | `pause` on radio is HTTP 500; app standby reads `powerOn` | `play` does not resume a paused player | HTTP server stalls for seconds |

## Shared lessons

- **Pause is often a toggle.** Read the state first and only send it when it moves the player the right way; a second pause shortly after the first must not undo it.
- **Live radio has no pause on StreamUnlimited.** The audioserver stops it and replays it on play.
- **A Cast stream can't be restarted by anyone but its Cast sender**, on any of these platforms.
- **Speakers report changes late.** After a volume step or a toggle, take the commanded value as current for a few seconds.
- **Never log or commit** stream URLs (Qobuz URLs are signed with the account id), account names, serials, MAC or home network addresses. Sanitize captures before adding them to [`tests/fixtures/speakers/`](../../tests/fixtures/speakers/).

## Checking a real speaker

Read-only status check from a terminal with local-network access:

```bash
STREAM800_LIVE=<ip>:kef cargo test live_status -- --ignored --nocapture
```

(`:naim` for a Mu-so.)

## KEF first generation

The LSX and LS50 Wireless (first generation) advertise only `_airplay._tcp`, with TXT `manufacturer=KEF`, `model=LSX` or `LS50W` and `deviceid=<MAC>` (the stable id). Discovery confirms them by opening TCP port 50001, where they speak a binary control protocol (documented by aiokef). The audioserver discovers and binds them, but has no backend for them yet.
