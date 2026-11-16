//! Per-zone 10-band equalizer preferences (in-memory; not applied to speaker playback).

use crate::player_state::Playerid;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use tracing::debug;

pub const BAND_COUNT: usize = 10;
pub const EQ_MIN: f32 = -10.0;
pub const EQ_MAX: f32 = 10.0;
pub const EQ_STEP: f32 = 0.5;

static BANDS: LazyLock<RwLock<HashMap<usize, [f32; BAND_COUNT]>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

static EQ_STORE_LOGGED: LazyLock<std::sync::atomic::AtomicBool> =
    LazyLock::new(|| std::sync::atomic::AtomicBool::new(false));

pub const BAND_NAMES: [&str; BAND_COUNT] = [
    "31 Hz", "63 Hz", "125 Hz", "250 Hz", "500 Hz", "1 kHz", "2 kHz", "4 kHz", "8 kHz", "16 kHz",
];

#[derive(Debug, Clone, Serialize)]
pub struct EqBandWire {
    pub id: usize,
    pub name: String,
    pub low: f32,
    pub high: f32,
    pub step: f32,
    pub value: f32,
}

pub fn get_bands(playerid: Playerid) -> [f32; BAND_COUNT] {
    BANDS
        .read()
        .expect("eq bands lock")
        .get(&playerid.0)
        .copied()
        .unwrap_or([0.0; BAND_COUNT])
}

pub fn bands_to_settings_string(bands: &[f32; BAND_COUNT]) -> String {
    bands
        .iter()
        .map(|v| format!("{:.1}", round_band(*v)))
        .collect::<Vec<_>>()
        .join(",")
}

pub fn parse_settings_csv(csv: &str) -> anyhow::Result<[f32; BAND_COUNT]> {
    let parts: Vec<&str> = csv
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() != BAND_COUNT {
        anyhow::bail!(
            "expected {BAND_COUNT} equalizer values, got {}",
            parts.len()
        );
    }
    let mut bands = [0.0; BAND_COUNT];
    for (i, part) in parts.iter().enumerate() {
        bands[i] = clamp_band(part.parse()?);
    }
    Ok(bands)
}

pub fn set_bands(playerid: Playerid, bands: [f32; BAND_COUNT]) {
    let clamped = clamp_bands(bands);
    BANDS
        .write()
        .expect("eq bands lock")
        .insert(playerid.0, clamped);
    log_store_once(playerid);
}

pub fn set_band(
    playerid: Playerid,
    band_index: usize,
    value: f32,
) -> anyhow::Result<[f32; BAND_COUNT]> {
    if band_index >= BAND_COUNT {
        anyhow::bail!("band index out of range: {band_index}");
    }
    let mut bands = get_bands(playerid);
    bands[band_index] = clamp_band(value);
    set_bands(playerid, bands);
    Ok(bands)
}

pub fn build_geteq_result(bands: &[f32; BAND_COUNT]) -> Vec<EqBandWire> {
    bands
        .iter()
        .enumerate()
        .map(|(id, &value)| EqBandWire {
            id,
            name: BAND_NAMES[id].to_string(),
            low: EQ_MIN,
            high: EQ_MAX,
            step: EQ_STEP,
            value: round_band(value),
        })
        .collect()
}

pub fn build_seteq_all_result(bands: &[f32; BAND_COUNT]) -> Value {
    let rows: Vec<Value> = bands
        .iter()
        .enumerate()
        .map(|(band, &value)| json!({ "band": band, "value": round_band(value) }))
        .collect();
    json!(rows)
}

pub fn build_seteq_single_result(playerid: Playerid, bands: &[f32; BAND_COUNT]) -> Value {
    let values: Vec<Value> = bands.iter().map(|v| json!(round_band(*v))).collect();
    json!({
        "id": playerid.0.to_string(),
        "name": "",
        "type": "equalizer",
        "values": values,
    })
}

pub fn build_equalizer_legacy_result(playerid: Playerid, bands: &[f32; BAND_COUNT]) -> Value {
    let mut equalizer = serde_json::Map::new();
    for (i, &v) in bands.iter().enumerate() {
        equalizer.insert(format!("B{i}"), json!(round_band(v)));
    }
    json!([{
        "playerid": playerid.0,
        "equalizer": equalizer,
    }])
}

fn clamp_bands(mut bands: [f32; BAND_COUNT]) -> [f32; BAND_COUNT] {
    for v in &mut bands {
        *v = clamp_band(*v);
    }
    bands
}

fn clamp_band(v: f32) -> f32 {
    v.clamp(EQ_MIN, EQ_MAX)
}

fn round_band(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

fn log_store_once(playerid: Playerid) {
    if !EQ_STORE_LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        debug!(
            player_id = playerid.0,
            "equalizer preference stored (not applied to speaker playback)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geteq_default_ten_flat_bands() {
        let bands = get_bands(Playerid(88_001));
        assert_eq!(bands, [0.0; BAND_COUNT]);
        let wire = build_geteq_result(&bands);
        assert_eq!(wire.len(), BAND_COUNT);
        assert_eq!(wire[0].name, "31 Hz");
        assert_eq!(wire[0].low, -10.0);
        assert_eq!(wire[0].step, 0.5);
    }

    #[test]
    fn parse_and_round_trip_csv() {
        let csv = "1.0,0,0,0,0,0,0,0,0,0";
        let bands = parse_settings_csv(csv).unwrap();
        assert_eq!(bands[0], 1.0);
        assert_eq!(bands[1], 0.0);
        assert_eq!(
            parse_settings_csv(&bands_to_settings_string(&bands)).unwrap(),
            bands
        );
    }

    #[test]
    fn set_band_updates_storage() {
        let id = Playerid(88_002);
        let bands = set_band(id, 3, 1.5).unwrap();
        assert_eq!(bands[3], 1.5);
        assert_eq!(get_bands(id)[3], 1.5);
    }

    #[test]
    fn parse_settings_csv_rejects_wrong_band_count() {
        assert!(parse_settings_csv("0,0,0,0,0,0,0,0,0").is_err());
        assert!(parse_settings_csv("0,0,0,0,0,0,0,0,0,0,0").is_err());
        assert!(parse_settings_csv("0,,0,0,0,0,0,0,0,0").is_err());
    }

    #[test]
    fn parse_settings_csv_rejects_non_numeric() {
        assert!(parse_settings_csv("bad,0,0,0,0,0,0,0,0,0").is_err());
    }

    #[test]
    fn parse_settings_csv_clamps_out_of_range() {
        let bands = parse_settings_csv("15,-15,0,0,0,0,0,0,0,0").unwrap();
        assert_eq!(bands[0], 10.0);
        assert_eq!(bands[1], -10.0);
    }
}
