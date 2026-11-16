# Loxone UI wire command appendix

Searchable index extracted from `scripts6.js` (`MediaEnum`), cross-referenced with [`src/remote/mod.rs`](../src/remote/mod.rs) routes and [`command-audit.log`](../command-audit.log).

Regenerate:

```bash
python3 tools/extract_ui_wire_commands.py
```

Machine-readable: [`ui-wire-commands.json`](ui-wire-commands.json) (generated 2026-06-07).

## Implementation legend

| Status | Meaning |
|--------|---------|
| `routed` | Handler in `src/remote/mod.rs` |
| `observed_not_routed` | In `command-audit.log` but no router match |
| `ui_only` | In UI bundle only |

## Browse-related config paths (scripts6.js literals)

| Wire template | Normalized | Status | Rust route(s) |
|---------------|------------|--------|---------------|
| `audio/cfg/getavailableservices` | `audio/cfg/getavailableservices` | observed_not_routed | — |
| `audio/cfg/getplaylists2/lms/%s/%s/0/50` | `audio/cfg/getplaylists2/lms/{param}/{param}/{n}/{n}` | ui_only | — |
| `audio/cfg/getradios` | `audio/cfg/getradios` | observed_not_routed | — |
| `audio/cfg/getservicefolder/%s/%s/%s/%i/%i` | `audio/cfg/getservicefolder/{param}/{param}/{param}/{param}/{param}` | ui_only | — |
| `audio/cfg/getservices` | `audio/cfg/getservices` | observed_not_routed | — |
| `audio/cfg/globalsearch/` | `audio/cfg/globalsearch/` | observed_not_routed | — |
| `audio/cfg/globalsearch/describe` | `audio/cfg/globalsearch/describe` | observed_not_routed | — |
| `audio/cfg/search` | `audio/cfg/search` | observed_not_routed | — |
| `audio/cfg/searchdetails/library/nouser/` | `audio/cfg/searchdetails/library/nouser/` | ui_only | — |
| `audio/cfg/searchresult/` | `audio/cfg/searchresult/` | ui_only | — |

## All config path literals (`audio/cfg/...`)

| Wire template | Browse | Status |
|---------------|--------|--------|
| `audio/cfg/airplay` |  | ui_only |
| `audio/cfg/airplay/disable` |  | ui_only |
| `audio/cfg/airplay/enable` |  | ui_only |
| `audio/cfg/audiodelay/%s` |  | ui_only |
| `audio/cfg/audiodelay/%s/%s` |  | ui_only |
| `audio/cfg/equalizer` |  | ui_only |
| `audio/cfg/favorites/addexternalid/` |  | ui_only |
| `audio/cfg/favorites/addid/` |  | ui_only |
| `audio/cfg/favorites/addpath/` |  | ui_only |
| `audio/cfg/favorites/delete/` |  | ui_only |
| `audio/cfg/favorites/rename/` |  | ui_only |
| `audio/cfg/getavailableservices` | yes | observed_not_routed |
| `audio/cfg/getinputs` |  | observed_not_routed |
| `audio/cfg/getkey` |  | observed_not_routed |
| `audio/cfg/getplayersdetails` |  | ui_only |
| `audio/cfg/getplaylists2/lms/%s/%s/0/50` | yes | ui_only |
| `audio/cfg/getradios` | yes | observed_not_routed |
| `audio/cfg/getservicefolder/%s/%s/%s/%i/%i` | yes | ui_only |
| `audio/cfg/getservices` | yes | observed_not_routed |
| `audio/cfg/getsyncedplayers` |  | ui_only |
| `audio/cfg/globalsearch/` | yes | observed_not_routed |
| `audio/cfg/globalsearch/describe` | yes | observed_not_routed |
| `audio/cfg/input/` |  | ui_only |
| `audio/cfg/installupdate` |  | ui_only |
| `audio/cfg/mac` |  | ui_only |
| `audio/cfg/playlist/create` |  | ui_only |
| `audio/cfg/playlist/deletelist` |  | ui_only |
| `audio/cfg/playlist/follow` |  | ui_only |
| `audio/cfg/playlist/rename` |  | ui_only |
| `audio/cfg/playlist/update` |  | observed_not_routed |
| `audio/cfg/props/updatelevel` |  | ui_only |
| `audio/cfg/rescan` |  | ui_only |
| `audio/cfg/rescan/%s` |  | ui_only |
| `audio/cfg/roomfavs/%s/copy/%s` |  | ui_only |
| `audio/cfg/scanstatus` |  | ui_only |
| `audio/cfg/search` | yes | observed_not_routed |
| `audio/cfg/searchdetails/library/nouser/` | yes | ui_only |
| `audio/cfg/searchresult/` | yes | ui_only |
| `audio/cfg/servicecfg/` |  | ui_only |
| `audio/cfg/servicecfg/delete/` |  | ui_only |
| `audio/cfg/servicecfg/getlink/` |  | ui_only |
| `audio/cfg/servicecfg/update/` |  | ui_only |
| `audio/cfg/spotifyconnect` |  | ui_only |
| `audio/cfg/spotifyconnect/disable` |  | ui_only |
| `audio/cfg/spotifyconnect/enable` |  | ui_only |
| `audio/cfg/storage/add/%s` |  | ui_only |
| `audio/cfg/storage/delete/%s` |  | ui_only |
| `audio/cfg/storage/edit/%s/%s` |  | ui_only |
| `audio/cfg/storage/list` |  | observed_not_routed |
| `audio/cfg/tunein/disable` |  | ui_only |
| `audio/cfg/tunein/enable` |  | ui_only |
| `audio/cfg/unsyncmulti/` |  | ui_only |

## `MediaEnum.Commands` config paths

| Enum path | Wire template | Browse | Status |
|-----------|---------------|--------|--------|
| `MediaEnum.Commands.SETTINGS.AIRPLAY.STATE` | `audio/cfg/airplay` |  | ui_only |
| `MediaEnum.Commands.SETTINGS.AIRPLAY.DISABLE` | `audio/cfg/airplay/disable` |  | ui_only |
| `MediaEnum.Commands.SETTINGS.AIRPLAY.ENABLE` | `audio/cfg/airplay/enable` |  | ui_only |
| `MediaEnum.Commands.SETTINGS.AUDIO_DELAY_GET` | `audio/cfg/audiodelay/%s` |  | ui_only |
| `MediaEnum.Commands.SETTINGS.AUDIO_DELAY_SET` | `audio/cfg/audiodelay/%s/%s` |  | ui_only |
| `MediaEnum.Commands.SETTINGS.EQ` | `audio/cfg/equalizer` |  | ui_only |
| `MediaEnum.Commands.FAVORITES.ADD_EXTERNAL_ID` | `audio/cfg/favorites/addexternalid/` |  | ui_only |
| `MediaEnum.Commands.FAVORITES.ADD_ID` | `audio/cfg/favorites/addid/` |  | ui_only |
| `MediaEnum.Commands.FAVORITES.ADD_PATH` | `audio/cfg/favorites/addpath/` |  | ui_only |
| `MediaEnum.Commands.FAVORITES.DELETE` | `audio/cfg/favorites/delete/` |  | ui_only |
| `MediaEnum.Commands.FAVORITES.RENAME` | `audio/cfg/favorites/rename/` |  | ui_only |
| `MediaEnum.Commands.SERVICES.GET_AVAILABLE` | `audio/cfg/getavailableservices` | yes | observed_not_routed |
| `MediaEnum.Commands.INPUT.GET` | `audio/cfg/getinputs` |  | observed_not_routed |
| `MediaEnum.Commands.GETKEY` | `audio/cfg/getkey` |  | observed_not_routed |
| `MediaEnum.Commands.AUDIO_STATUS_ALL` | `audio/cfg/getplayersdetails` |  | ui_only |
| `MediaEnum.Commands.PLAYLIST.GET_PLAYLIST_2` | `audio/cfg/getplaylists2/lms/%s/%s/0/50` | yes | ui_only |
| `MediaEnum.Commands.RADIOS.GET` | `audio/cfg/getradios` | yes | observed_not_routed |
| `MediaEnum.Commands.PLAYLIST.GET_SERVICE_FOLDER` | `audio/cfg/getservicefolder/%s/%s/%s/%i/%i` | yes | ui_only |
| `MediaEnum.Commands.SERVICES.GET` | `audio/cfg/getservices` | yes | observed_not_routed |
| `MediaEnum.Commands.SYNC.GET_SYNCED_PLAYERS` | `audio/cfg/getsyncedplayers` |  | ui_only |
| `MediaEnum.Commands.GLOBAL_SEARCH.BASE_CMD` | `audio/cfg/globalsearch/` | yes | observed_not_routed |
| `MediaEnum.Commands.GLOBAL_SEARCH.DESCRIBE` | `audio/cfg/globalsearch/describe` | yes | observed_not_routed |
| `MediaEnum.Commands.INPUT.SET.CMD` | `audio/cfg/input/` |  | ui_only |
| `MediaEnum.Commands.INSTALLUPDATE` | `audio/cfg/installupdate` |  | ui_only |
| `MediaEnum.Commands.GETMAC` | `audio/cfg/mac` |  | ui_only |
| `MediaEnum.Commands.PLAYLIST.CREATE` | `audio/cfg/playlist/create` |  | ui_only |
| `MediaEnum.Commands.PLAYLIST.DELETE` | `audio/cfg/playlist/deletelist` |  | ui_only |
| `MediaEnum.Commands.PLAYLIST.FOLLOW` | `audio/cfg/playlist/follow` |  | ui_only |
| `MediaEnum.Commands.PLAYLIST.RENAME` | `audio/cfg/playlist/rename` |  | ui_only |
| `MediaEnum.Commands.PLAYLIST.UPDATE` | `audio/cfg/playlist/update` |  | observed_not_routed |
| `MediaEnum.Commands.UPD_LVL` | `audio/cfg/props/updatelevel` |  | ui_only |
| `MediaEnum.Commands.LIBRARY.RESCAN_ALL` | `audio/cfg/rescan` |  | ui_only |
| `MediaEnum.Commands.LIBRARY.RESCAN_SPECIFIC` | `audio/cfg/rescan/%s` |  | ui_only |
| `MediaEnum.Commands.COPY_ZONE_FAVS` | `audio/cfg/roomfavs/%s/copy/%s` |  | ui_only |
| `MediaEnum.Commands.LIBRARY.SCAN_STATUS` | `audio/cfg/scanstatus` |  | ui_only |
| `MediaEnum.Commands.SEARCH.SELECTIVE` | `audio/cfg/search` | yes | observed_not_routed |
| `MediaEnum.Commands.SEARCH.BROWSE` | `audio/cfg/searchresult/` | yes | ui_only |
| `MediaEnum.Commands.SERVICES.CFG.GET` | `audio/cfg/servicecfg/` |  | ui_only |
| `MediaEnum.Commands.SERVICES.CFG.SET` | `audio/cfg/servicecfg/` |  | ui_only |
| `MediaEnum.Commands.SERVICES.CFG.DELETE` | `audio/cfg/servicecfg/delete/` |  | ui_only |
| `MediaEnum.Commands.SERVICES.CFG.GET_LINK` | `audio/cfg/servicecfg/getlink/` |  | ui_only |
| `MediaEnum.Commands.SERVICES.CFG.UPDATE` | `audio/cfg/servicecfg/update/` |  | ui_only |
| `MediaEnum.Commands.SETTINGS.SPOTIFY.CONNECT.STATE` | `audio/cfg/spotifyconnect` |  | ui_only |
| `MediaEnum.Commands.SETTINGS.SPOTIFY.CONNECT.DISABLE` | `audio/cfg/spotifyconnect/disable` |  | ui_only |
| `MediaEnum.Commands.SETTINGS.SPOTIFY.CONNECT.ENABLE` | `audio/cfg/spotifyconnect/enable` |  | ui_only |
| `MediaEnum.Commands.SETTINGS.LIBRARY.ADD` | `audio/cfg/storage/add/%s` |  | ui_only |
| `MediaEnum.Commands.SETTINGS.LIBRARY.DELETE` | `audio/cfg/storage/delete/%s` |  | ui_only |
| `MediaEnum.Commands.SETTINGS.LIBRARY.EDIT` | `audio/cfg/storage/edit/%s/%s` |  | ui_only |
| `MediaEnum.Commands.SETTINGS.LIBRARY.LIST` | `audio/cfg/storage/list` |  | observed_not_routed |
| `MediaEnum.Commands.SERVICES.RADIO.DISABLE` | `audio/cfg/tunein/disable` |  | ui_only |
| `MediaEnum.Commands.SERVICES.RADIO.ENABLE` | `audio/cfg/tunein/enable` |  | ui_only |
| `MediaEnum.Commands.SYNC.MULTI_UNSYNC` | `audio/cfg/unsyncmulti/` |  | ui_only |

## Zone wire templates (`audio/{playerid}/…` suffixes from `MediaEnum.AudioCommands`)

Built at runtime as `audio/` + `playerid` + `/` + suffix unless the value is already a full `audio/cfg/…` path.

| Enum path | Wire template | Category |
|-----------|---------------|----------|
| `MediaEnum.AudioCommands.FAVORITE.PLAY` | `audio/{playerid}/favoriteplay/` | browse_or_play |
| `MediaEnum.AudioCommands.LIBRARY.PLAY` | `audio/{playerid}/library/play/` | browse_or_play |
| `MediaEnum.AudioCommands.PLAYLIST.PLAY` | `audio/{playerid}/playlist/play/` | browse_or_play |
| `MediaEnum.AudioCommands.SEARCH.ADD` | `audio/{playerid}/addurl/` | browse_or_play |
| `MediaEnum.AudioCommands.SEARCH.INSERT` | `audio/{playerid}/inserturl/` | browse_or_play |
| `MediaEnum.AudioCommands.SEARCH.INSERT_AND_PLAY` | `audio/{playerid}/insertandplayurl/` | browse_or_play |
| `MediaEnum.AudioCommands.SEARCH.PLAY` | `audio/{playerid}/playurl/` | browse_or_play |
| `MediaEnum.AudioCommands.SERVICE.ADD` | `audio/{playerid}/serviceplayadd/` | browse_or_play |
| `MediaEnum.AudioCommands.SERVICE.INSERT` | `audio/{playerid}/serviceplayinsert/` | browse_or_play |
| `MediaEnum.AudioCommands.SERVICE.INSERT_AND_PLAY` | `audio/{playerid}/serviceplayinsertandplay/` | browse_or_play |
| `MediaEnum.AudioCommands.SERVICE.PLAY` | `audio/{playerid}/serviceplay/` | browse_or_play |
| `MediaEnum.AudioCommands.ZONE_FAV.DELETE` | `audio/{playerid}/roomfav/delete/` | browse_or_play |
| `MediaEnum.AudioCommands.ZONE_FAV.PLAY` | `audio/{playerid}/roomfav/play/` | browse_or_play |
| `MediaEnum.AudioCommands.ZONE_FAV.SAVE` | `audio/{playerid}/roomfav/save/` | browse_or_play |
| `MediaEnum.AudioCommands.ZONE_FAV.SAVE_EXTERNAL` | `audio/{playerid}/roomfav/saveexternalid/` | browse_or_play |
| `MediaEnum.AudioCommands.ZONE_FAV.SAVE_INPUT` | `audio/{playerid}/roomfav/saveinput/` | browse_or_play |
| `MediaEnum.AudioCommands.ZONE_FAV.SAVE_LOCAL` | `audio/{playerid}/roomfav/saveid/` | browse_or_play |
| `MediaEnum.AudioCommands.ZONE_FAV.SAVE_PATH` | `audio/{playerid}/roomfav/savepath/` | browse_or_play |

### All zone command suffixes

| Enum path | Wire template | Category |
|-----------|---------------|----------|
| `MediaEnum.AudioCommands.CFG.AUDIO_DELAY` | `audio/{playerid}/audiodelay` | other |
| `MediaEnum.AudioCommands.CFG.DEFAULT_VOLUME` | `audio/{playerid}/defaultvolume` | other |
| `MediaEnum.AudioCommands.CFG.EQUALIZER` | `audio/{playerid}/equalizer` | other |
| `MediaEnum.AudioCommands.CFG.MAX_VOLUME` | `audio/{playerid}/maxvolume` | other |
| `MediaEnum.AudioCommands.FAVORITE.PLAY` | `audio/{playerid}/favoriteplay/` | browse_or_play |
| `MediaEnum.AudioCommands.GET_SYNCED_PLAYERS` | `audio/{playerid}/getmysyncedplayers` | other |
| `MediaEnum.AudioCommands.IDENTIFY_SOURCE` | `audio/{playerid}/identifysource` | other |
| `MediaEnum.AudioCommands.LIBRARY.PLAY` | `audio/{playerid}/library/play/` | browse_or_play |
| `MediaEnum.AudioCommands.LINEIN.PLAY` | `audio/{playerid}/linein` | other |
| `MediaEnum.AudioCommands.MASTER_VOLUME.REQUEST` | `audio/{playerid}/mastervolume` | other |
| `MediaEnum.AudioCommands.MASTER_VOLUME.SET_TO` | `audio/{playerid}/mastervolume/` | other |
| `MediaEnum.AudioCommands.PAUSE` | `audio/{playerid}/pause` | transport |
| `MediaEnum.AudioCommands.PLAY` | `audio/{playerid}/play` | transport |
| `MediaEnum.AudioCommands.PLAYLIST.PLAY` | `audio/{playerid}/playlist/play/` | browse_or_play |
| `MediaEnum.AudioCommands.QUEUE.ADD` | `audio/{playerid}/queueadd/` | transport |
| `MediaEnum.AudioCommands.QUEUE.CLEAR` | `audio/{playerid}/queue/clear` | transport |
| `MediaEnum.AudioCommands.QUEUE.GET` | `audio/{playerid}/getqueue` | transport |
| `MediaEnum.AudioCommands.QUEUE.INSERT` | `audio/{playerid}/queueinsert/` | transport |
| `MediaEnum.AudioCommands.QUEUE.INSERT_AND_PLAY` | `audio/{playerid}/queueandplay/` | transport |
| `MediaEnum.AudioCommands.QUEUE.JUMP_TO` | `audio/{playerid}/queue/` | transport |
| `MediaEnum.AudioCommands.QUEUE.MOVE` | `audio/{playerid}/queuemove/` | transport |
| `MediaEnum.AudioCommands.QUEUE.NEXT` | `audio/{playerid}/queueplus` | transport |
| `MediaEnum.AudioCommands.QUEUE.PREV` | `audio/{playerid}/queueminus` | transport |
| `MediaEnum.AudioCommands.QUEUE.REMOVE` | `audio/{playerid}/queueremove/` | transport |
| `MediaEnum.AudioCommands.QUEUE.SAVE_AS_PLAYLIST` | `audio/{playerid}/playlist/save/` | transport |
| `MediaEnum.AudioCommands.QUEUE.UNDO` | `audio/{playerid}/queueundo/` | transport |
| `MediaEnum.AudioCommands.QUEUE_V2.MOVE_BEFORE` | `audio/{playerid}/queue/move/%s/before/%s` | other |
| `MediaEnum.AudioCommands.QUEUE_V2.PLAY_ID` | `audio/{playerid}/queue/play/%s` | other |
| `MediaEnum.AudioCommands.QUEUE_V2.REMOVE` | `audio/{playerid}/queue/remove/%s` | other |
| `MediaEnum.AudioCommands.REPEAT.OFF` | `audio/{playerid}/repeat/0` | transport |
| `MediaEnum.AudioCommands.REPEAT.PLAYLIST` | `audio/{playerid}/repeat/1` | transport |
| `MediaEnum.AudioCommands.REPEAT.TRACK` | `audio/{playerid}/repeat/3` | transport |
| `MediaEnum.AudioCommands.SEARCH.ADD` | `audio/{playerid}/addurl/` | browse_or_play |
| `MediaEnum.AudioCommands.SEARCH.INSERT` | `audio/{playerid}/inserturl/` | browse_or_play |
| `MediaEnum.AudioCommands.SEARCH.INSERT_AND_PLAY` | `audio/{playerid}/insertandplayurl/` | browse_or_play |
| `MediaEnum.AudioCommands.SEARCH.PLAY` | `audio/{playerid}/playurl/` | browse_or_play |
| `MediaEnum.AudioCommands.SEEK.BACKWARD_BY` | `audio/{playerid}/position/-` | transport |
| `MediaEnum.AudioCommands.SEEK.FORWARD_BY` | `audio/{playerid}/position/+` | transport |
| `MediaEnum.AudioCommands.SEEK.TO` | `audio/{playerid}/position/` | transport |
| `MediaEnum.AudioCommands.SERVICE.ADD` | `audio/{playerid}/serviceplayadd/` | browse_or_play |
| `MediaEnum.AudioCommands.SERVICE.INSERT` | `audio/{playerid}/serviceplayinsert/` | browse_or_play |
| `MediaEnum.AudioCommands.SERVICE.INSERT_AND_PLAY` | `audio/{playerid}/serviceplayinsertandplay/` | browse_or_play |
| `MediaEnum.AudioCommands.SERVICE.PLAY` | `audio/{playerid}/serviceplay/` | browse_or_play |
| `MediaEnum.AudioCommands.SHUFFLE.OFF` | `audio/{playerid}/shuffle/0` | transport |
| `MediaEnum.AudioCommands.SHUFFLE.ON` | `audio/{playerid}/shuffle/1` | transport |
| `MediaEnum.AudioCommands.STATUS` | `audio/{playerid}/status` | transport |
| `MediaEnum.AudioCommands.SYNC` | `audio/{playerid}/sync/` | other |
| `MediaEnum.AudioCommands.UNSYNC` | `audio/{playerid}/unsync` | other |
| `MediaEnum.AudioCommands.VOLUME.DOWN` | `audio/{playerid}/voldown` | transport |
| `MediaEnum.AudioCommands.VOLUME.DOWN_BY` | `audio/{playerid}/volume/-` | transport |
| `MediaEnum.AudioCommands.VOLUME.REQUEST` | `audio/{playerid}/volume` | transport |
| `MediaEnum.AudioCommands.VOLUME.SET_TO` | `audio/{playerid}/volume/` | transport |
| `MediaEnum.AudioCommands.VOLUME.UP` | `audio/{playerid}/volup` | transport |
| `MediaEnum.AudioCommands.VOLUME.UP_BY` | `audio/{playerid}/volume/+` | transport |
| `MediaEnum.AudioCommands.ZONE_FAV.DELETE` | `audio/{playerid}/roomfav/delete/` | browse_or_play |
| `MediaEnum.AudioCommands.ZONE_FAV.PLAY` | `audio/{playerid}/roomfav/play/` | browse_or_play |
| `MediaEnum.AudioCommands.ZONE_FAV.SAVE` | `audio/{playerid}/roomfav/save/` | browse_or_play |
| `MediaEnum.AudioCommands.ZONE_FAV.SAVE_EXTERNAL` | `audio/{playerid}/roomfav/saveexternalid/` | browse_or_play |
| `MediaEnum.AudioCommands.ZONE_FAV.SAVE_INPUT` | `audio/{playerid}/roomfav/saveinput/` | browse_or_play |
| `MediaEnum.AudioCommands.ZONE_FAV.SAVE_LOCAL` | `audio/{playerid}/roomfav/saveid/` | browse_or_play |
| `MediaEnum.AudioCommands.ZONE_FAV.SAVE_PATH` | `audio/{playerid}/roomfav/savepath/` | browse_or_play |

## Rust routes (`src/remote/mod.rs`)

| Route | Browse |
|-------|--------|

## Observed commands (`command-audit.log`)

| Command | Browse |
|---------|--------|
| `audio/1/getqueue/0/50` | yes |
| `audio/1/pause` |  |
| `audio/1/play` |  |
| `audio/1/position/140` |  |
| `audio/1/position/48` |  |
| `audio/1/position/81` |  |
| `audio/1/queueminus` |  |
| `audio/1/queueplus` |  |
| `audio/1/recent` | yes |
| `audio/1/repeat/1` |  |
| `audio/1/volume/+1` |  |
| `audio/1/volume/-1` |  |
| `audio/1/volume/10` |  |
| `audio/1/volume/11` |  |
| `audio/3/getqueue/0/50` | yes |
| `audio/3/loudness` |  |
| `audio/3/pause` |  |
| `audio/3/playlist/play/qobuz:track:422318101` | yes |
| `audio/3/playlist/play/qobuz:track:422318101/noshuffle` | yes |
| `audio/3/playlist/play/qobuz:track:422318103` | yes |
| `audio/3/playlist/play/qobuz:track:422318103/noshuffle` | yes |
| `audio/3/playlist/play/qobuz:track:422318105` | yes |
| `audio/3/playlist/play/qobuz:track:422318105/noshuffle` | yes |
| `audio/3/recent` | yes |
| `audio/3/serviceplay/spotify/qobuz/qobuz:album:trrcz9pvaaz6b/?q&ZW5mb3JjZVVzZXI9dHJ1ZQ` | yes |
| `audio/3/serviceplay/spotify/qobuz/qobuz:playlist:64085790/?q&ZW5mb3JjZVVzZXI9dHJ1ZQ` | yes |
| `audio/3/serviceplay/spotify/qobuz/qobuz:playlist:64085790/noshuffle/?q&ZW5mb3JjZVVzZXI9dHJ1ZQ` | yes |
| `audio/3/volume/+1` |  |
| `audio/3/volume/-1` |  |
| `audio/4/recent` | yes |
| `audio/cfg/diagnosis` |  |
| `audio/cfg/getavailableservices` | yes |
| `audio/cfg/getconfig` |  |
| `audio/cfg/geteq/3` |  |
| `audio/cfg/getinputs` |  |
| `audio/cfg/getkey/full` |  |
| `audio/cfg/getmediafolder/0/0/50` | yes |
| `audio/cfg/getplaylists2/lms/nouser/0/0/20` | yes |
| `audio/cfg/getplaylists2/lms/nouser/0/0/50` | yes |
| `audio/cfg/getplaylists2/lms/nouser/qobuz:track:422318104/0/50` | yes |
| `audio/cfg/getradios` | yes |
| `audio/cfg/getroomfavs/1/0/50` | yes |
| `audio/cfg/getroomfavs/3/0/50` | yes |
| `audio/cfg/getroomfavs/4/0/50` | yes |
| `audio/cfg/getroomfavs/all` | yes |
| `audio/cfg/getservicefolder/custom/custom/start/0/20` | yes |
| `audio/cfg/getservicefolder/custom/custom/start/0/50` | yes |
| `audio/cfg/getservicefolder/local/local/start/0/20` | yes |
| `audio/cfg/getservicefolder/local/local/start/0/50` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/0/0/20` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/1/0/20` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/2/0/20` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/3/0/20` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/3/0/50` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/6/0/50` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/6/100/50` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/6/50/50` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/genres:112/0/50` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/genres:80/0/50` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/qobuz:album:euwm523d8n0x0/0/50` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/qobuz:album:m4tdldaf9n99d/0/50` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/qobuz:album:trrcz9pvaaz6b/0/50` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/qobuz:artist:2489921/0/50` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/qobuz:artist:43839/0/50` | yes |
| `audio/cfg/getservicefolder/spotify/qobuz/qobuz:playlist:64085790/0/50` | yes |
| `audio/cfg/getservices` | yes |
| `audio/cfg/globalsearch/describe` | yes |
| `audio/cfg/globalsearch/local:track#5,album#5,artist#5,playlist#5,folder#5/hello` | yes |
| `audio/cfg/globalsearch/spotify@qobuz:track#5,album#5,artist#5,playlist#5,episode#5,show#5/Rem` | yes |
| `audio/cfg/globalsearch/spotify@qobuz:track#5,album#5,artist#5,playlist#5,episode#5,show#5/radiohea` | yes |
| `audio/cfg/globalsearch/spotify@qobuz:track#5,album#5,artist#5,playlist#5,episode#5,show#5/radiohead` | yes |
| `audio/cfg/globalsearch/spotify@qobuz:track#5,album#5,artist#5,playlist#5,episode#5,show#5/t` | yes |
| `audio/cfg/globalsearch/spotify@qobuz:track#5,album#5,artist#5,playlist#5,episode#5,show#5/tin` | yes |
| `audio/cfg/globalsearch/spotify@qobuz:track#5,album#5,artist#5,playlist#5,episode#5,show#5/tinderstick` | yes |
| `audio/cfg/globalsearch/spotify@qobuz:track#5,album#5,artist#5,playlist#5,episode#5,show#5/tindersticks` | yes |
| `audio/cfg/globalsearch/tunein:station#5,custom#5/hellosd` | yes |
| `audio/cfg/globalsearch/tunein:station#5,custom#5/radio` | yes |
| `audio/cfg/groupopts/eyJncm91cHMiOltdfQ` |  |
| `audio/cfg/isfollowed/spotify/qobuz/qobuz:album:euwm523d8n0x0` |  |
| `audio/cfg/isfollowed/spotify/qobuz/qobuz:artist:2489921` |  |
| `audio/cfg/isfollowed/spotify/qobuz/qobuz:track:33933680` |  |
| `audio/cfg/miniserverip/192.168.60.12/255.255.255.0/192.168.60.1/192.168.60.1,195.121.1.66/192.168.50.204/dhcp` |  |
| `audio/cfg/miniserverip/192.168.60.12/255.255.255.0/192.168.60.1/192.168.60.1,195.121.1.66/192.168.50.238/dhcp` |  |
| `audio/cfg/miniservertime/1779295656` |  |
| `audio/cfg/miniservertime/1779295666` |  |
| `audio/cfg/miniservertime/1779302663` |  |
| `audio/cfg/miniservertime/1779302673` |  |
| `audio/cfg/miniservertime/1780832148` |  |
| `audio/cfg/miniservertime/1780832156` |  |
| `audio/cfg/miniservertime/1780833056` |  |
| `audio/cfg/miniservertime/1780833683` |  |
| `audio/cfg/miniservertime/1780833691` |  |
| `audio/cfg/miniservertime/1780834304` |  |
| `audio/cfg/miniservertime/1780834312` |  |
| `audio/cfg/miniservertime/1780834402` |  |
| `audio/cfg/miniservertime/1780834410` |  |
| `audio/cfg/miniservertime/1780835311` |  |
| `audio/cfg/miniservertime/1780835519` |  |
| `audio/cfg/miniservertime/1780835526` |  |
| `audio/cfg/miniservertime/1780836427` |  |
| `audio/cfg/miniservertime/1780836571` |  |
| `audio/cfg/miniservertime/1780836579` |  |
| `audio/cfg/miniservertime/1780836680` |  |
| `audio/cfg/miniservertime/1780836688` |  |
| `audio/cfg/miniservertime/1780847292` |  |
| `audio/cfg/miniservertime/1780847300` |  |
| `audio/cfg/miniservertime/1780847918` |  |
| `audio/cfg/miniservertime/1780847926` |  |
| `audio/cfg/miniservertime/1780848693` |  |
| `audio/cfg/miniservertime/1780848700` |  |
| `audio/cfg/miniservertime/1780849477` |  |
| `audio/cfg/miniservertime/1780849485` |  |
| `audio/cfg/miniservertime/1780850080` |  |
| `audio/cfg/miniservertime/1780850088` |  |
| `audio/cfg/miniservertime/1780850989` |  |
| `audio/cfg/miniservertime/1780851891` |  |
| `audio/cfg/miniservertime/1780852794` |  |
| `audio/cfg/miniservertime/1780853697` |  |
| `audio/cfg/miniserverversion/17.0.3.31/17.0.3.31` |  |
| `audio/cfg/playername/eyJwbGF5ZXJzIjpbeyJwbGF5ZXJpZCI6MSwibmFtZSI6IlV0aWxpdHkgUm9vbSJ9XX0` |  |
| `audio/cfg/playername/eyJwbGF5ZXJzIjpbeyJwbGF5ZXJpZCI6MywibmFtZSI6IkJlZHJvb20ifSx7InBsYXllcmlkIjo0LCJuYW1lIjoiQmFsY29ueSJ9XX0` |  |
| `audio/cfg/playeropts/eyJwbGF5ZXJzIjpbeyJwbGF5ZXJpZCI6MSwicmYiOiJvZmYiLCJsb2NrIjoib2ZmIiwiYXAiOiJvbiIsInNwIjoib24ifV19` |  |
| `audio/cfg/playeropts/eyJwbGF5ZXJzIjpbeyJwbGF5ZXJpZCI6MywicmYiOiJvZmYiLCJsb2NrIjoib2ZmIiwiYXAiOiJvZmYiLCJzcCI6Im9mZiJ9LHsicGxheWVyaWQiOjQsInJmIjoib2ZmIiwibG9jayI6Im9mZiIsImFwIjoib2ZmIiwic3AiOiJvZmYifV19` |  |
| `audio/cfg/playlist/update/lms/nouser/qobuz:track:422318104/start` |  |
| `audio/cfg/presencemode/1/off` |  |
| `audio/cfg/presencemode/3,4/off` |  |
| `audio/cfg/ready` |  |
| `audio/cfg/search/spotify/qobuz/album/radiohead/0/50` | yes |
| `audio/cfg/setconfigtimestamp/1779215107` |  |
| `audio/cfg/setconfigtimestamp/1780830003` |  |
| `audio/cfg/speakertype/eyJzcGVha2VycyI6W3siaWQiOiI1MDRGOTRGRjFCQjMjMSIsInNwZWFrZXJUeXBlIjozfSx7ImlkIjoiNTA0Rjk0RkYxQkIzIzMiLCJzcGVha2VyVHlwZSI6M31dfQ` |  |
| `audio/cfg/speakertype/eyJzcGVha2VycyI6W3siaWQiOiI4RTA5RjdENUFEREIjMSIsInNwZWFrZXJUeXBlIjozfV19` |  |
| `audio/cfg/storage/list` |  |
| `audio/cfg/timezone/Europe/Vienna` |  |
| `audio/cfg/volumes/eyJwbGF5ZXJzIjpbeyJwbGF5ZXJpZCI6MSwiZGVmYXVsdCI6MTAsImFsYXJtIjo3NSwiZmlyZSI6NzUsImJlbGwiOjM1LCJidXp6ZXIiOjEwLCJ0dHMiOjIwLCJ2b2xzdGVwIjoxLCJmYWRpbmciOjAsIm1heFZvbHVtZSI6MTAwfV19` |  |
| `audio/cfg/volumes/eyJwbGF5ZXJzIjpbeyJwbGF5ZXJpZCI6MSwiZGVmYXVsdCI6MTAsImFsYXJtIjo3NSwiZmlyZSI6NzUsImJlbGwiOjM1LCJidXp6ZXIiOjEwLCJ0dHMiOjIxLCJ2b2xzdGVwIjoxLCJmYWRpbmciOjAsIm1heFZvbHVtZSI6MTAwfV19` |  |
| `audio/cfg/volumes/eyJwbGF5ZXJzIjpbeyJwbGF5ZXJpZCI6MSwiZGVmYXVsdCI6MTAsImFsYXJtIjo3NSwiZmlyZSI6NzUsImJlbGwiOjM1LCJidXp6ZXIiOjExLCJ0dHMiOjIwLCJ2b2xzdGVwIjoxLCJmYWRpbmciOjAsIm1heFZvbHVtZSI6MTAwfV19` |  |
| `audio/cfg/volumes/eyJwbGF5ZXJzIjpbeyJwbGF5ZXJpZCI6MSwiZGVmYXVsdCI6MTAsImFsYXJtIjo3NSwiZmlyZSI6NzUsImJlbGwiOjM2LCJidXp6ZXIiOjEwLCJ0dHMiOjIwLCJ2b2xzdGVwIjoxLCJmYWRpbmciOjAsIm1heFZvbHVtZSI6MTAwfV19` |  |
| `audio/cfg/volumes/eyJwbGF5ZXJzIjpbeyJwbGF5ZXJpZCI6MSwiZGVmYXVsdCI6MTAsImFsYXJtIjo3NiwiZmlyZSI6NzYsImJlbGwiOjM1LCJidXp6ZXIiOjEwLCJ0dHMiOjIwLCJ2b2xzdGVwIjoxLCJmYWRpbmciOjAsIm1heFZvbHVtZSI6MTAwfV19` |  |
| `audio/cfg/volumes/eyJwbGF5ZXJzIjpbeyJwbGF5ZXJpZCI6MSwiZGVmYXVsdCI6MTEsImFsYXJtIjo3NSwiZmlyZSI6NzUsImJlbGwiOjM1LCJidXp6ZXIiOjEwLCJ0dHMiOjIwLCJ2b2xzdGVwIjoxLCJmYWRpbmciOjAsIm1heFZvbHVtZSI6MTAwfV19` |  |
| `audio/cfg/volumes/eyJwbGF5ZXJzIjpbeyJwbGF5ZXJpZCI6MywiZGVmYXVsdCI6MTAsImFsYXJtIjo3NSwiZmlyZSI6NzUsImJlbGwiOjM1LCJidXp6ZXIiOjEwLCJ0dHMiOjIwLCJ2b2xzdGVwIjoxLCJmYWRpbmciOjAsIm1heFZvbHVtZSI6MTAwfSx7InBsYXllcmlkIjo0LCJkZWZhdWx0IjoxMCwiYWxhcm0iOjc1LCJmaXJlIjo3NSwiYmVsbCI6MzUsImJ1enplciI6MTAsInR0cyI6MjAsInZvbHN0ZXAiOjEsImZhZGluZyI6MCwibWF4Vm9sdW1lIjoxMDB9XX0` |  |
| `audio/grouped/tts/1/ENU|20` |  |
| `audio/grouped/tts/1/ENU|21` |  |

## Feature gates (`MediaEnum.Features`)

Relevant entries for browse/search:

| Feature | API | Firmware |
|---------|-----|----------|
| `GETMEDIAFOLDER_ERROR` | 1.1 | 1.0.10.8 |
| `GLOBAL_SEARCH` | 1.6 | 2.3.09.02 |
| `LIBRARY_PLAY` | 1.1 | 0.0.6.9 |
| `NAS_LIBRARY` | 1.6 | 2.3.10.15 |
| `NEW_LOGIN_HANDLING` | 1.3 | 1.1.4.20 |
| `NEW_PLAYLISTS` | 1.6 | 1.1.12.03 |
| `NEW_SERVICE_PLAY` | 1.3 | 1.1.6.9 |
| `RENAME_PLAYLIST` | 1.6 | 2.4.1.23 |
| `SEARCHABLE_GENRES` | 3.0 | 9.1.11.29 |
| `SEARCHABLE_PLAYLISTS` | 1.6 | 1.1.12.03 |
| `SEARCH_ALL` | 9.9 | 99.99.99.99 |
| `SEARCH_LIB_IMPROVED` | 1.3 | 1.0.11.27 |
| `SEARCH_RADIO` | 1.1 | 0.0.7.21 |
| `SEARCH_SERVICES_IMPROVED` | 1.4 | 1.0.11.27 |
