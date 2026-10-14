//! Talking to the Junctur script running inside Resolve: a request file in the mailbox, the
//! answer in `Fusion.prefs` ([`super::reply`]). One request at a time, since the mailbox holds one
//! file. The waiting is a loop over a "read the prefs" function so it is tested without Resolve.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::lua::{self, Value};
use super::reply;

/// No ack this long after writing a request: no script is reading the mailbox.
pub const ACK: Duration = Duration::from_secs(3);
/// Placing thousands of clips keeps Resolve busy for minutes.
pub const ANSWER: Duration = Duration::from_secs(300);
const POLL: Duration = Duration::from_millis(50);

pub const NOT_LISTENING: &str = "Resolve is not listening: open DaVinci Resolve and choose \
     Workspace > Scripts > Junctur (once per Resolve session), then send again";

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// `<config folder>\resolve-bridge`, beside `config.json`.
pub fn mailbox() -> Option<PathBuf> {
    crate::config::folder().map(|f| f.join("resolve-bridge"))
}

/// Resolve's per-user support folder; it exists once Resolve has run.
pub fn support() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(|a| {
        PathBuf::from(a)
            .join("Blackmagic Design")
            .join("DaVinci Resolve")
            .join("Support")
    });
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|h| {
        PathBuf::from(h).join("Library/Application Support/Blackmagic Design/DaVinci Resolve")
    });
    #[cfg(all(unix, not(target_os = "macos")))]
    let base =
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/DaVinciResolve"));
    base
}

/// The newest `Fusion\Profiles\*\Fusion.prefs`: the profile in use is the one Resolve writes.
fn prefs() -> Option<PathBuf> {
    let profiles = support()?.join("Fusion").join("Profiles");
    std::fs::read_dir(profiles)
        .ok()?
        .flatten()
        .map(|e| e.path().join("Fusion.prefs"))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

/// A request id: milliseconds since 1970 then a counter, so the script can tell a request left
/// over from long ago (it ignores those older than two minutes) and two in one millisecond differ.
fn next_id() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    format!(
        "{}",
        ms * 1000 + COUNTER.fetch_add(1, Ordering::Relaxed) % 1000
    )
}

/// Sends request `kind` with `fields` and waits for the answer. Errors are the user's to read.
pub fn request(
    kind: &str,
    fields: Vec<(&'static str, Value)>,
) -> Result<HashMap<String, String>, String> {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let dir = mailbox().ok_or("no folder for Junctur's settings")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let id = next_id();
    let file = dir.join("request.lua");
    write_request(&file, &id, kind, fields)?;
    let read = || prefs().and_then(|p| std::fs::read_to_string(p).ok());
    let answer = wait(&id, read, ACK, ANSWER);
    // Gone once answered (or given up on), so a script started later never runs it.
    let _ = std::fs::remove_file(&file);
    let answer = answer?;
    match reply::error(&answer) {
        Some(e) => Err(e),
        None => Ok(answer),
    }
}

fn write_request(
    file: &Path,
    id: &str,
    kind: &str,
    mut fields: Vec<(&'static str, Value)>,
) -> Result<(), String> {
    fields.insert(0, ("kind", kind.into()));
    fields.insert(0, ("id", id.into()));
    // Written beside and renamed over, so the script never loads half a file.
    let tmp = file.with_extension("lua.tmp");
    std::fs::write(&tmp, lua::chunk(&Value::Table(fields)))
        .and_then(|()| std::fs::rename(&tmp, file))
        .map_err(|e| format!("{}: {e}", file.display()))
}

/// Polls `read` (the prefs text) for request `id`'s ack within `ack`, then its answer within
/// `answer` of the ack.
pub fn wait(
    id: &str,
    read: impl Fn() -> Option<String>,
    ack: Duration,
    answer: Duration,
) -> Result<HashMap<String, String>, String> {
    let begun = Instant::now();
    let mut acked_at = None;
    loop {
        if let Some(text) = read() {
            if let Some(a) = reply::answer(&text, id) {
                return Ok(a);
            }
            if acked_at.is_none() && reply::acked(&text, id) {
                acked_at = Some(Instant::now());
            }
        }
        match acked_at {
            None if begun.elapsed() > ack => return Err(NOT_LISTENING.into()),
            Some(at) if at.elapsed() > answer => {
                return Err(
                    "Resolve took the request but has not answered; it may still be \
                            working, look at the timeline before sending again"
                        .into(),
                )
            }
            _ => std::thread::sleep(POLL),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn hex(s: &str) -> String {
        s.bytes().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn waits_for_the_ack_then_the_answer() {
        let calls = Cell::new(0);
        let read = || {
            calls.set(calls.get() + 1);
            Some(match calls.get() {
                0..=2 => String::new(),
                3..=5 => "JuncturAck = \"7\"".to_string(),
                _ => format!(
                    "JuncturAck = \"7\" JuncturReply = \"7:{}\"",
                    hex("placed\t2")
                ),
            })
        };
        let a = wait("7", read, Duration::from_secs(5), Duration::from_secs(5)).unwrap();
        assert_eq!(a["placed"], "2");
    }

    #[test]
    fn no_ack_means_nobody_is_listening() {
        // An old request's ack in the file is not this one's.
        let read = || Some("JuncturAck = \"6\"".to_string());
        let e = wait(
            "7",
            read,
            Duration::from_millis(120),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert_eq!(e, NOT_LISTENING);
        let e = wait(
            "7",
            || None,
            Duration::from_millis(60),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert_eq!(e, NOT_LISTENING);
    }

    #[test]
    fn an_ack_with_no_answer_gives_up_after_its_own_wait() {
        let read = || Some("JuncturAck = \"7\"".to_string());
        let e = wait(
            "7",
            read,
            Duration::from_millis(50),
            Duration::from_millis(120),
        )
        .unwrap_err();
        assert!(e.contains("has not answered"), "{e}");
    }

    #[test]
    fn the_request_file_is_a_lua_chunk_with_its_id_and_kind_first() {
        let d = std::env::temp_dir().join(format!("junctur-resolve-bridge-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("request.lua");
        write_request(&f, "12", "info", vec![("template", "".into())]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "return {[\"id\"] = \"12\", [\"kind\"] = \"info\", [\"template\"] = \"\"}\n"
        );
        assert!(!f.with_extension("lua.tmp").exists());
        let _ = std::fs::remove_dir_all(&d);
        assert_ne!(next_id(), next_id());
    }
}
