//! TuneIn browse/search, custom streams, and radio audiopath helpers.

mod audiopath;
mod custom_streams;
mod search_cache;
mod state;
mod tunein_api;
mod tunein_browse;
mod tunein_resolve;
mod tunein_search;

pub use audiopath::{is_radio_audiopath, is_radio_service, parse_serviceplay_item_id};
pub use custom_streams::{CustomRadioError, CustomRadioStore};
pub use search_cache::remember_tunein_query;
pub use state::build_radio_player_state;
pub use tunein_api::station_metadata;
pub use tunein_browse::browse_service_folder;
pub use tunein_resolve::{resolve_radio_audiopath, ResolvedRadioStream};
pub use tunein_search::{
    build_radio_search_wire_payload, global_search_tunein, parse_tunein_provider_spec,
    search_category_tunein,
};
