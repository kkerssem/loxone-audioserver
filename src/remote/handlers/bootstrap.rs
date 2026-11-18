//! Service bootstrap and cfg stub handlers.
use anyhow::Context;
use matchit::Params;
use serde_json::json;
use tokio::sync::oneshot;

use crate::wire_response::build_wire_result_response;

use super::super::{Api, BrowseOpts, BrowseResult, IoCommand};
use tracing::debug;

pub(in crate::remote) fn format_mac_address(serial: &str) -> String {
    let hex: String = serial.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let upper = hex.to_uppercase();
    if upper.len() == 12 {
        return (0..6)
            .map(|i| &upper[i * 2..i * 2 + 2])
            .collect::<Vec<_>>()
            .join(":");
    }
    upper
}

impl Api {
    pub(in crate::remote) fn get_inputs(&self, command: &str) -> anyhow::Result<String> {
        /*
         * Command: audio/cfg/getinputs
         *
         * Priority: LOW
         */
        build_wire_result_response("getinputs_result", json!([]), Some(command), &[])
    }

    pub(in crate::remote) async fn get_media_folder(
        &self,
        command: &str,
        params: &Params<'_, '_>,
    ) -> anyhow::Result<String> {
        /*
         * Command: audio/cfg/getmediafolder/{id}/{start}/{max_items}
         * - id: Local media folder id. `"0"` is the SD card / on-device storage root
         *   (radio catalog browse uses `getservicefolder`).
         *   Subfolder ids come from prior `getmediafolder` item `id` values.
         * - start: The first item to retrieve (from the total list).
         * - max_items: The maximum number of items to retrieve. Typically 50.
         */
        let id = params.get("id").context("Missing parameter id")?;
        let start: usize = params
            .get("start")
            .context("Missing parameter start")?
            .parse()?;
        let (sender, receiver) = oneshot::channel();
        let opts = BrowseOpts {
            id: id.to_string(),
            sender,
            session_token: self.session_token.clone(),
            start,
        };
        self.sender.send(IoCommand::GetMediaFolder(opts)).await?;
        let browse_result = match receiver.await {
            Ok(browse_result) => browse_result,
            Err(error) => {
                debug!(?error, "sender dropped");
                BrowseResult {
                    id: id.to_string(),
                    items: vec![],
                    name: String::from(""),
                    start,
                    service: None,
                    total_items: 0,
                }
            }
        };
        build_wire_result_response(
            "getmediafolder_result",
            json!([browse_result]),
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) async fn get_radios(&self, command: &str) -> anyhow::Result<String> {
        /*
         * Command: audio/cfg/getradios
         *
         * Reply:
         * - An array of items with the following payload:
         *   [{
         *     cmd: String -> Used for call to audio/cfg/getservicefolder/{cmd}/noUser/0/0/50
         *                    also used for showing different icons Radios.LOCAL, Radios.WORLD, Radios.CUSTOM,...?
         *     icon: String -> Can be custom url, or if not determined by cmd value?
         *     root: String -> ?
         *     name: String -> The name of the folder
         *   },...]
         *
         * Approach:
         * - The top level getradios call seems to be followed by the
         *   getservicefolder call after selecting a top level item.
         *   So maybe I just need to return these here.
         *   One could then be "My Live Radio", and the other could be
         *   "Live Radio". For both the browse approach should probably work.
         */
        // let (sender, receiver) = oneshot::channel();
        // self.sender.send(IoCommand::GetRadios(sender)).await?;
        // let radios = match receiver.await {
        //     Ok(radios) => radios,
        //     Err(_error) => RadioList { items: vec![] },
        // };

        let icon_tunein = "https://extended-app-content.s3.eu-central-1.amazonaws.com/audioZone/services/Icon-TuneIn.svg";
        build_wire_result_response(
            "getradios_result",
            json!([
                {
                    "cmd": "local",
                    "name": "TuneIn Presets",
                    "icon": icon_tunein,
                    "root": "start",
                },
                {
                    "cmd": "custom",
                    "name": "Custom Streams",
                    "icon": icon_tunein,
                    "root": "start",
                    "editable": true,
                }
            ]),
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) fn get_room_favs(
        &self,
        command: &str,
        params: &Params,
    ) -> anyhow::Result<String> {
        self.get_room_favs_page(command, params)
    }

    pub(in crate::remote) fn get_mac(&self, command: &str) -> anyhow::Result<String> {
        let mac = format_mac_address(&self.app_config.serial);
        let entry = json!({ "macaddress": mac });
        build_wire_result_response(
            "mac_result",
            json!([entry.clone()]),
            Some(command),
            &[("data", json!([entry]))],
        )
    }

    pub(in crate::remote) async fn get_players_details(
        &self,
        command: &str,
    ) -> anyhow::Result<String> {
        let (sender, receiver) = oneshot::channel();
        self.sender.send(IoCommand::AllPlayerStatus(sender)).await?;
        let states = receiver.await.unwrap_or_default();
        let states_val = serde_json::to_value(&states)?;
        build_wire_result_response(
            "getplayersdetails_result",
            states_val.clone(),
            Some(command),
            &[("data", states_val)],
        )
    }

    pub(in crate::remote) fn props_update_level(&self, command: &str) -> anyhow::Result<String> {
        build_wire_result_response("props_result", json!(true), Some(command), &[])
    }

    /// No streaming accounts: playback is started from each speaker's own app.
    pub(in crate::remote) fn get_services(&self, command: &str) -> anyhow::Result<String> {
        build_wire_result_response("getservices_result", json!([]), Some(command), &[])
    }

    pub(in crate::remote) fn get_available_services(
        &self,
        command: &str,
    ) -> anyhow::Result<String> {
        build_wire_result_response("getavailableservices_result", json!([]), Some(command), &[])
    }

    pub(in crate::remote) async fn diagnosis(&self, command: &str) -> anyhow::Result<String> {
        /*
         * Command: audio/cfg/diagnosis
         * The Loxone App shows `diagnosis_result` (an object) as indented JSON text.
         */
        let (reply, rx) = oneshot::channel();
        self.sender.send(IoCommand::Diagnosis(reply)).await?;
        let report = rx.await.context("diagnosis reply dropped")?;
        build_wire_result_response("diagnosis_result", report, Some(command), &[])
    }

    pub(in crate::remote) fn identify(&self, command: &str) -> anyhow::Result<String> {
        /*
         * Command: audio/cfg/identify
         * UI uses mac + services to show Audioserver presence.
         */
        build_wire_result_response(
            "identify_result",
            json!({
                "mac": self.app_config.serial,
                "services": [],
            }),
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) fn storage_list(&self, command: &str) -> anyhow::Result<String> {
        /*
         * Command: audio/cfg/storage/list
         */
        build_wire_result_response("storage_result", json!([]), Some(command), &[])
    }

    pub(in crate::remote) fn get_synced_players(&self, command: &str) -> anyhow::Result<String> {
        /*
         * Order at startup: 4
         * Command: audio/cfg/getsyncedplayers
         *
         * Approach:
         * - Can be hooked up to grouping functionality in Roon.
         * - A bit more complex when not all devices are Roon-ready.
         *
         * Priority: LOW
         */
        build_wire_result_response("getsyncedplayers_result", json!([]), Some(command), &[])
    }

    pub(in crate::remote) fn not_implemented(&self, command: &str) -> anyhow::Result<String> {
        crate::loxone_wire::not_implemented_reply(command)
    }

    pub(in crate::remote) fn rescan(&self, command: &str) -> anyhow::Result<String> {
        build_wire_result_response("rescan_result", json!([]), Some(command), &[])
    }

    pub(in crate::remote) fn scan_status(&self, command: &str) -> anyhow::Result<String> {
        /*
         * Order at startup: 3
         * Command: audio/cfg/scanstatus
         */
        build_wire_result_response("scanstatus_result", json!([]), Some(command), &[])
    }
}

#[cfg(test)]
mod tests {
    use super::format_mac_address;

    #[test]
    fn format_mac_address_colon_separated() {
        assert_eq!(format_mac_address("504F94FF1BB3"), "50:4F:94:FF:1B:B3");
        assert_eq!(format_mac_address("abc"), "ABC");
    }
}
