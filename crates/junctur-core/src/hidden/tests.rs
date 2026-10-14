use super::*;

pub fn scratch(name: &str) -> PathBuf {
    crate::test_support::scratch("hidden", name)
}

/// Writes a file modified a minute ago (settled), or now when `fresh`.
pub fn write(p: &Path, s: &str, fresh: bool) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
    if !fresh {
        set_modified(p, SystemTime::now() - Duration::from_secs(60)).unwrap();
    }
}

/// A fake AppData under `d`: the lookup, the real Local, and packages A's and B's hidden Local.
pub fn appdata(d: &Path) -> (Lookup, PathBuf, PathBuf, PathBuf) {
    let local = d.join("Local");
    let a = local.join("Packages/A_1/LocalCache/Local");
    let b = local.join("Packages/B_2/LocalCache/Local");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    (Lookup::new(&local, Some(&d.join("Roaming"))), local, a, b)
}

#[test]
fn finds_each_packages_copy_of_a_local_or_roaming_folder() {
    let d = scratch("copies");
    let (local, roaming) = (d.join("Local"), d.join("Roaming"));
    let pkgs = local.join("Packages");
    fs::create_dir_all(pkgs.join("A_1/LocalCache/Local/junctur/cache")).unwrap();
    fs::create_dir_all(pkgs.join("B_2/LocalCache/Local/junctur")).unwrap();
    fs::create_dir_all(pkgs.join("B_2/LocalCache/Roaming/junctur")).unwrap();
    let l = Lookup::new(&local, Some(&roaming));

    assert_eq!(
        l.copies(&local.join("junctur/cache")),
        vec![pkgs.join("A_1/LocalCache/Local/junctur/cache")]
    );
    assert_eq!(
        l.copies(&roaming.join("junctur")),
        vec![pkgs.join("B_2/LocalCache/Roaming/junctur")]
    );
    // Outside AppData, AppData itself, and the Packages folder have none.
    assert!(l.copies(&d.join("elsewhere")).is_empty());
    assert!(l.copies(&local).is_empty());
    assert!(l.copies(&pkgs.join("A_1")).is_empty());
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_differently_cased_or_verbatim_folder_finds_its_copies() {
    let d = scratch("case");
    let (l, local, a, _) = appdata(&d);
    fs::create_dir_all(a.join("junctur/cache")).unwrap();
    let shouted = PathBuf::from(local.to_string_lossy().to_uppercase()).join("junctur/cache");
    assert_eq!(l.copies(&shouted), vec![a.join("junctur/cache")]);
    if cfg!(windows) {
        assert_eq!(
            relative(
                Path::new(r"\\?\C:\Users\U\AppData\Local\junctur"),
                Path::new(r"c:\users\u\appdata\local")
            ),
            Some(PathBuf::from("junctur"))
        );
    }
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn takes_settled_missing_files_once_and_never_replaces_the_users() {
    let d = scratch("files");
    let (l, local, a, b) = appdata(&d);
    let real = local.join("junctur");
    write(&a.join("junctur/voices.json"), "hidden voices", false);
    write(&a.join("junctur/config.json"), "hidden config", false);
    write(&a.join("junctur/recent.json"), "being written", true);
    write(&a.join("junctur/voices.json.tmp"), "half", false);
    write(&b.join("junctur/config.json"), "the other copy's", false);
    write(&real.join("voices.json"), "the user's voices", false);
    let all = |_: &Path| true;

    let first = adopt_files_with(&l, &real, all, SystemTime::now());
    assert_eq!(
        first,
        Adopted {
            copied: 1,
            differ: 1,
            pending: 1
        }
    );
    let read = |p: &str| fs::read_to_string(real.join(p)).ok();
    assert_eq!(read("config.json").as_deref(), Some("hidden config"));
    assert_eq!(read("voices.json").as_deref(), Some("the user's voices"));
    assert_eq!(read("recent.json"), None, "not settled yet");
    assert_eq!(read("voices.json.tmp"), None);
    assert!(files(&real).iter().all(|f| !is_temporary(f)));
    assert_eq!(
        fs::read_to_string(a.join("junctur/voices.json")).unwrap(),
        "hidden voices",
        "the hidden copy is untouched"
    );

    // A taken config the user deletes does not come back, from either copy; the settled
    // recent.json comes.
    fs::remove_file(real.join("config.json")).unwrap();
    let later = SystemTime::now() + Duration::from_secs(60);
    assert_eq!(adopt_files_with(&l, &real, all, later).copied, 1);
    assert_eq!(read("config.json"), None);
    assert_eq!(read("recent.json").as_deref(), Some("being written"));

    // Only what `keep` accepts.
    let other = local.join("other");
    write(&a.join("other/x.json"), "x", false);
    write(&a.join("other/y.json"), "y", false);
    adopt_files_with(&l, &other, |r| r == Path::new("y.json"), SystemTime::now());
    assert!(!other.join("x.json").exists() && other.join("y.json").exists());
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_copy_keeps_the_time_and_never_replaces() {
    let d = scratch("copy");
    let (src, dst) = (d.join("src.json"), d.join("dst.json"));
    write(&src, "first", false);
    assert!(copy_in(&src, &dst).unwrap());
    assert_eq!(stamp(&src), stamp(&dst), "the modified time kept");
    fs::write(&dst, "the user's").unwrap();
    assert!(!copy_in(&src, &dst).unwrap());
    assert_eq!(fs::read_to_string(&dst).unwrap(), "the user's");
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_folder_without_hidden_copies_is_left_alone() {
    let d = scratch("none");
    let (l, local, _, _) = appdata(&d);
    let real = local.join("junctur");
    assert_eq!(
        adopt_files_with(
            &l,
            &real,
            |_| panic!("nothing to ask about"),
            SystemTime::now()
        ),
        Adopted::default()
    );
    assert!(!real.exists(), "not even a ledger");
    fs::remove_dir_all(&d).unwrap();
}
