//! The Lua script Junctur installs in Resolve, run against a stand-in Resolve (`resolve_script.lua`).
//! Needs a Lua 5.1 interpreter (`JUNCTUR_LUA`, `luajit`, `lua` or Resolve's `fuscript`); without one
//! it says so and passes, since a machine without Resolve cannot run the script anyway.

use std::path::{Path, PathBuf};
use std::process::Command;

fn interpreter() -> Option<(PathBuf, Vec<&'static str>)> {
    if let Some(p) = std::env::var_os("JUNCTUR_LUA") {
        return Some((p.into(), vec![]));
    }
    for name in ["luajit", "lua", "lua5.1"] {
        if Command::new(name).arg("-v").output().is_ok() {
            return Some((name.into(), vec![]));
        }
    }
    let fuscript = Path::new(r"C:\Program Files\Blackmagic Design\DaVinci Resolve\fuscript.exe");
    fuscript
        .is_file()
        .then(|| (fuscript.to_path_buf(), vec!["-l", "lua"]))
}

#[test]
fn the_resolve_script_places_restyles_and_removes_against_a_stand_in() {
    let Some((lua, args)) = interpreter() else {
        eprintln!("no Lua interpreter (set JUNCTUR_LUA): the Resolve script was not run");
        return;
    };
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = here.join("src").join("resolve").join("junctur.lua");
    let mock = here.join("tests").join("resolve_script.lua");
    // The interpreters disagree on passing arguments and on globals between files, so the
    // script's path goes at the top of a copy of the stand-in.
    let dir = std::env::temp_dir().join(format!("junctur-lua-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let run = dir.join("run.lua");
    std::fs::write(
        &run,
        format!(
            "local SCRIPT = [==[{}]==]\n{}",
            script.display(),
            std::fs::read_to_string(&mock).unwrap()
        ),
    )
    .unwrap();
    let out = Command::new(&lua).args(args).arg(&run).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("ALL OK"), "{}:\n{text}", lua.display());
}
