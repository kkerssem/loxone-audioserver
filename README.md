# Loxone Audioserver

Software-only [Loxone Audioserver](https://www.loxone.com/) for LAN speakers. Loxone zones get transport, volume and now-playing from the speakers themselves; you pick what to play in each speaker's own app (or the streaming service's app). TuneIn and custom radio can be started from the Loxone App and are played through Google Cast.

Runs as a single daemon: Miniserver pairing on port **7095**, Web UI on port **7091**, UDP discovery on **7070**.

> **Status:** the Qobuz Connect controller was removed in November 2026 after its upstream crates (`vicrodh/qbz`) were taken offline. KEF W2-platform speakers (LS60, LSX II, LS50 Wireless II) and Naim Mu-so 2nd gen are controlled through their local API: now-playing, play/pause, volume and wake-on-play. First-generation KEF (LSX, LS50 Wireless) is not supported yet.

## Prerequisites

- **Rust** (latest stable) — [rustup](https://rustup.rs/)
- **Loxone Miniserver** + Config
- For radio: a **Google Cast** speaker on the LAN

## Build and run

```bash
cargo build --release
./target/release/loxone-audioserver --serial 504F94FF1BB3 --name "Loxone Audioserver"
```

Or via Cargo:

```bash
cargo run --release -- --serial 504F94FF1BB3 --name "Loxone Audioserver"
```

Optional flags: `--hostname`, `--public-host` (cover art host; default is auto-detected LAN IPv4).

## Loxone onboarding

Wire protocol references: [Miniserver pairing (7095)](docs/miniserver-protocol.md), [Web UI (7091)](docs/web-ui-protocol.md), [7091 vs 7095 routing](docs/traffic-paths.md).

### 1. Start the daemon

Run it from the directory that should hold **`audioserver-config.json`** (zone mappings and the Loxone config snapshot).

### 2. Add in Loxone Config

- **Search:** daemon answers UDP probes on 7070 (reply from 7091; JSON field `s` is bare 12-hex serial, e.g. `504F94FF1BB3`).
- **Manual:** host IP + port **7095** for Miniserver pairing (UDP advertises TCP **7091** for Config, same as hardware).

Assign zones/players; the Miniserver sends `setconfig` with `playerid` values, and the daemon creates one row per zone in `audioserver.players[]`.

### 3. Map zones to speakers

Edit `audioserver.players[]` in `audioserver-config.json`:

| Field | Description |
|-------|-------------|
| `player_id` | Loxone `playerid` from Miniserver `setconfig` |
| `loxone_uuid` | Audioplayer uuid from `loxone.state` (auto-synced on `setconfig`) |
| `name` | Zone display name (auto-synced) |
| `cast_device_id` | Google Cast device id, needed for radio (see below) |
| `speaker_id` | Speaker to control for play/pause, volume, power and now-playing (see below) |
| `speaker_host` | Optional manual IP/host for the speaker (overrides discovery) |
| `speaker_kind` | Needed only with `speaker_host` when mDNS cannot see the speaker: `kef_stream800`, `naim_stream800`, `kef_legacy` or `beo` (Bang & Olufsen BeoLink, e.g. Beoplay M3; its API is on port 8080 of that host) |

**Speakers** are found on the LAN via mDNS and logged at startup as `speaker discovered` with their `speaker_id`. A zone whose Loxone name equals a speaker's name (for example a zone called "Mu-so Qb") is bound automatically and the `speaker_id` is saved; otherwise copy the id from the log into that zone's row. Bang & Olufsen speakers are found via `_beocore._tcp`; their `speaker_id` is the B&O id (`<type>.<item>.<serial>@products.bang-olufsen.com`).

Find a Cast device id on macOS:

```bash
dns-sd -B _googlecast._tcp
```

then `dns-sd -L "<instance name>" _googlecast._tcp` and copy the 32-hex `id=` value from the TXT record. Details: [docs/radio-cast.md](docs/radio-cast.md).

Configs written by older Qobuz builds still load; their Qobuz keys are ignored and dropped on the next save.

### 4. Control from the Loxone App

Open the Web/App UI on the same LAN. Media commands may arrive on **`ws://<audioserver-ip>:7091`** (`remotecontrol`) and/or **7095** — see [traffic-paths.md](docs/traffic-paths.md).

**Pairing:** cert-chain echo on `secure/hello`, fixed `LEGACY_JWT` on `secure/init` (same as reference lox-audioserver). See [miniserver-protocol.md](docs/miniserver-protocol.md#initial-onboarding-loxone-config-add).

**Album art:** `coverurl` in `audio_event` uses `http://<audioserver-host>:7091/cors-proxy?...`. Override the host with `--public-host <ip-or-hostname>`.

**TLS:** not wired yet — plain WebSocket on 7091/7095. [`src/tls.rs`](src/tls.rs) is a stub.

| UI area | Wire command | Behaviour |
|---------|--------------|-----------|
| **Radios** | `getradios`, `getservicefolder/local/…`, `globalsearch/tunein…` | TuneIn / custom streams, played via Cast |
| **Music services** | `getservices` | Empty — start music from the speaker's app |
| **Queue panel** | `getqueue` | Empty |
| **Library** (SD / NAS) | `getmediafolder/0` | **Not implemented** |

## Configuration

`audioserver-config.json` has two sections: `loxone` (the Miniserver `setconfig` snapshot and its CRC) and `audioserver` (`players[]` with zone mappings, volume presets and room favorites).

## Debugging

```bash
RUST_LOG=loxone_audioserver=debug cargo run --release -- --serial YOUR_SERIAL
LOXONE_AUDIT=1 cargo run --release -- --serial YOUR_SERIAL   # wire commands to command-audit.log
```

Default filter (when `RUST_LOG` is unset): `info`, with `mdns_sd=off`. Setting `RUST_LOG` without `mdns_sd=` auto-appends `mdns_sd=off`.

Optional: `PROBE_SESSION_KEYS_DECRYPT=1` for a verbose `session_keys` decrypt probe (expected to fail with self-generated keys — non-fatal).

## Container image (ARM64)

CI builds a minimal **linux/arm64** image (one static binary on `scratch`) and pushes it to GitHub Container Registry on pushes to `main` and `v*` tags:

`ghcr.io/<owner>/loxone-audioserver:latest`, `:<7-character commit>`, and `:1.2.3` for a `v1.2.3` tag.

Build it locally with `container build --platform linux/arm64 -t loxone-audioserver .`, or push it by hand with [`scripts/push-image.sh`](scripts/push-image.sh).

Mount a single volume at `/data` so config and runtime data survive restarts:

| Path under `/data` | Purpose |
|--------------------|---------|
| `audioserver-config.json` | Zone mappings, volume prefs, room favorites |
| `private.pem` | Miniserver pairing key (auto-created on first run) |
| `custom-radios.json` | Custom radio station list |
| `custom-radio-covers/` | Uploaded radio cover images |
| `recents/` | Per-zone recently played lists |
| `voice-uploads/` | Voice messages recorded in the Loxone App |

With Apple's `container` CLI:

```bash
container run -d --name loxone-audioserver \
  -p 7091:7091 -p 7095:7095 -p 7070:7070/udp \
  -v loxone-data:/data \
  -e AUDIOSERVER_SERIAL=504F94FF1BB3 -e AUDIOSERVER_PUBLIC_HOST=<host-ip> \
  ghcr.io/<owner>/loxone-audioserver:latest
```

Settings are environment variables: `AUDIOSERVER_SERIAL` (required), `AUDIOSERVER_NAME` (the image sets `Loxone Audioserver`), `AUDIOSERVER_HOSTNAME` and `AUDIOSERVER_PUBLIC_HOST` match the `--serial`, `--name`, `--hostname` and `--public-host` options, which can also be given after the image name. `LOG_TIMESTAMPS=false` (or `no`, `0`, `off`) leaves the timestamp off log lines for runtimes that add their own. The image's health check runs `loxone-audioserver health`, which checks that ports 7091 and 7095 accept connections.

Cast and speaker discovery use mDNS, which only reaches the LAN when the container is on it: with host networking on a Linux host, or with its own LAN address as in the MikroTik App below. Apple's `container` runs containers on a separate virtual network, so there set `speaker_host` and `speaker_kind` per zone.

## MikroTik App

On a MikroTik router with RouterOS 7.26 or later and the container package, the audioserver runs as an App. [mikrotik/loxone-audioserver.yaml](mikrotik/loxone-audioserver.yaml) defines it: one service with a `data` volume, on its own VETH on your LAN bridge. That gives the audioserver its own address on the LAN, which it needs for mDNS discovery of speakers and Cast devices, for Loxone Config's UDP discovery on 7070, for the Miniserver's connection to 7095, and for Cast devices fetching covers and voice messages from 7091. No ports are forwarded.

The file contains no secrets. The Miniserver pairing key (`private.pem`) is created on first start and kept in the volume.

### Steps

1. **Make the image public, once.** CI pushes `ghcr.io/kkerssem/loxone-audioserver`, but a new package on ghcr.io is private, and Apps pull without a registry login (a private package fails with `auth error`). Make it public under your GitHub profile → Packages → `loxone-audioserver` → Package settings → Change visibility. The image contains only the binary and CA certificates.

2. **Add the app**, from the raw GitHub URL of the YAML file or after uploading it to the router:

   ```routeros
   /app/settings/set disk=usb1 lan-bridge=<your-bridge>
   /app/add yaml=[/file/get loxone-audioserver.yaml contents]
   # only for a VLAN on a vlan-filtering bridge; leave out for the untagged LAN (PVID 1)
   /app/set loxone-audioserver network-pvid=<vlan-id>
   # no RouterOS reverse proxy: the app has no web page for it, and with use-https=yes it never starts
   /app/set loxone-audioserver use-https=no
   ```

3. **Set the serial** (`AUDIOSERVER_SERIAL` in the app's environment) before `/app/enable loxone-audioserver`. Use the serial of your current audioserver if you have one: it is what Loxone Config knows the audioserver by. Until it is set, the service stops at startup with `set AUDIOSERVER_SERIAL (or --serial) …` in the log.

4. **Make the address static.** The Miniserver configuration stores the audioserver's IP, so make the VETH's DHCP lease static. The lease matches the router-side `mac-address`, not `container-mac-address`:

   ```routeros
   /interface/veth/print detail where name~"veth-app-loxone-audioserver"
   /ip/dhcp-server/lease/make-static [find where host-name~"veth-app-loxone-audioserver"]
   ```

   If this is a new address for an existing audioserver, update it in Loxone Config.

5. **Check it runs:** the log shows `loxone audioserver daemon running` with your serial, and the health check (`loxone-audioserver health`) turns healthy once ports 7091 and 7095 accept connections.

### Moving an existing install

To keep the zone mappings and the pairing, stop the old audioserver and copy its `audioserver-config.json`, `private.pem`, `custom-radios.json`, `custom-radio-covers/` and `recents/` into the app's `data` volume on the router's disk (find it with `/file/print where name~"apps/loxone-audioserver"`), then restart the app. Keep the same serial.

### Network notes

- **VLAN:** the VLAN must be in `/interface/bridge/vlan` with the bridge as a tagged member; RouterOS adds the app's port as untagged based on `network-pvid`. The speakers, Cast devices and Miniserver should be on that VLAN too, since mDNS and the UDP discovery broadcast do not cross VLANs.
- **Multicast:** with IGMP snooping on the bridge, check that mDNS reaches the app's VETH: with `RUST_LOG` set to `info,loxone_audioserver::speaker=debug,loxone_audioserver::cast=debug` the log shows the speakers and Cast devices it discovers.
- **Outgoing access:** leave `network-outgoing-access` at `yes` (the default); `no` also cuts the audioserver off from the speakers, Cast devices and internet radio.

## Development

[AGENTS.md](AGENTS.md) · [CONTRIBUTING.md](CONTRIBUTING.md) · [docs/README.md](docs/README.md)

```bash
cargo test
cargo clippy -- -D warnings
just test / just check / just audit   # if just installed
```

CI (`.github/workflows/ci.yml`): Rust format, instrumented tests/coverage/audit on every push; Clippy on PRs only; ARM64 Docker image to private GHCR.

## Project status

| Area | Status |
|------|--------|
| Loxone Config pairing (7095) | Done |
| Loxone Web UI protocol (7091 and/or 7095) | Done |
| TuneIn / custom radio via Google Cast | Done |
| Room favorites, recents, EQ / loudness preferences | Done |
| Speaker discovery, zone binding, polling and transport routing | Done |
| KEF W2 platform (LS60) and Naim Mu-so 2nd gen backends | Done (verified on LS60 and Mu-so Qb 2) |
| First-generation KEF (LSX, LS50 Wireless) backend | **Not yet** |
| Power on/off from Loxone | Wake-on-play only. Loxone sends no power command: the Audio Player **Off** input arrives as a plain `pause`, so speakers go to standby on their own auto-standby timer (set it in the KEF Connect / Naim app) |
| Qobuz Connect controller | Removed (upstream gone) |
| Local file library / SD / NAS, LMS playlists | **Not implemented** |

See [docs/architecture.md](docs/architecture.md).

## Troubleshooting

**Transport has no effect**
Only zones with a working backend respond: a Cast radio session started from Loxone, or a bound speaker. Other zones log `transport ignored: no backend for zone` at debug level; bound first-generation KEF speakers log `no backend for speaker kind yet`.

**Turning a zone off only pauses it**
The Audio Player **Off** input (e.g. wired to "leaving room") reaches the audioserver as `audio/{id}/pause`, identical to the App's pause button. The speaker then enters standby on its own auto-standby timer; shorten that timer in the KEF Connect or Naim app if you want it to switch off sooner. Play in Loxone wakes it again, but after standby the speaker has no queue left to resume, so start the music from the KEF / Naim / Qobuz app.

**Naim inputs (HDMI, optical, …) show as Wi-Fi**
The Mu-so's physical input is not read yet (it would cost an extra request per poll to Naim's port 15081 API), so a TV or optical source appears without the input name. KEF inputs are shown.

**Internet radio shows only the station name**
KEF and Naim report the station for their built-in radio, not the current song, so Loxone shows the station as title.

**Radio does not start**
Check `cast_device_id` for that zone in `audioserver-config.json` and that the speaker is on the same LAN segment (mDNS).
