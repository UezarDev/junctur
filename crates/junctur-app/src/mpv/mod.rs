//! libmpv, loaded at runtime: the raw API ([`ffi`]), a safe handle, events, the render API, and
//! the parsing of what mpv reports. Nothing here knows about Tauri or windows.

mod event;
pub mod ffi;
mod handle;
pub mod locate;
mod render;
mod tracks;

pub use event::{EndReason, Event, Value};
pub use handle::{Events, Format, Mpv};
pub use render::RenderContext;
pub use tracks::{audio_tracks, AudioTrack};

use std::sync::Arc;

/// Loads libmpv from the first place [`locate::find`] finds it.
pub fn load() -> Result<Arc<ffi::Api>, String> {
    let path = locate::find()?;
    let api = ffi::Api::load(&path)?;
    // SAFETY: a plain getter.
    let version = unsafe { (api.client_api_version)() };
    // The structs in ffi.rs follow client API 2.x; a 1.x DLL lays some of them out differently.
    if version >> 16 != 2 {
        return Err(format!(
            "{} has client API {}.{}, expected 2.x",
            path.display(),
            version >> 16,
            version & 0xffff
        ));
    }
    Ok(Arc::new(api))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wrapper against the real library, without a window: options, a command, an observed
    /// property and events. Needs `scripts/fetch-libmpv.ps1` first, so it is ignored by default.
    #[test]
    #[ignore = "needs libmpv (scripts/fetch-libmpv.ps1)"]
    fn plays_a_generated_source_headless() {
        let opts = [("vo", "null"), ("ao", "null"), ("config", "no")];
        let mpv = Arc::new(Mpv::new(load().unwrap(), &opts).unwrap());
        let mut events = mpv.events().unwrap();
        assert!(mpv.events().is_none(), "only one event reader");
        mpv.observe(1, "duration", Format::Double).unwrap();
        mpv.command(&["loadfile", "av://lavfi:testsrc=d=2"])
            .unwrap();
        assert!(mpv.command(&["no-such-command"]).is_err());
        let (mut loaded, mut duration) = (false, None);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while (!loaded || duration.is_none()) && std::time::Instant::now() < deadline {
            match events.wait(1.0) {
                Some(Event::FileLoaded) => loaded = true,
                Some(Event::Property {
                    id: 1,
                    value: Value::Double(d),
                }) => duration = Some(d),
                _ => {}
            }
        }
        assert!(loaded);
        // lavfi sources report an estimate, not exactly 2 s.
        assert!(duration.unwrap() > 0.0);
        mpv.set_string("pause", "yes").unwrap();
        assert!(mpv.set_string("no-such-property", "1").is_err());
    }
}
