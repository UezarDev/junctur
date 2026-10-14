use super::*;

fn info() -> VideoInfo {
    VideoInfo {
        codec: "h264".into(),
        profile: "Main".into(),
        level: 40,
        pix_fmt: "yuv420p".into(),
        color_range: Some("tv".into()),
        color_space: Some("bt709".into()),
        fps_num: 30,
        fps_den: 1,
        avg_num: 30,
        avg_den: 1,
        bit_rate: Some(5_000_000),
        b_frames: true,
        ..Default::default()
    }
}

fn after<'a>(a: &'a [String], flag: &str) -> Option<&'a str> {
    let i = a.iter().position(|x| x == flag)?;
    a.get(i + 1).map(String::as_str)
}

#[test]
fn copy_piece_seeks_to_its_keyframe_and_lands_at_its_place() {
    let p = Piece {
        start: 120,
        end: 360,
        at: 20,
        copy: true,
    };
    let a = args(Path::new("in.mp4"), &[p], &info(), true, Path::new("o.ts"));
    assert_eq!(after(&a, "-ss"), Some("4.000000"));
    assert_eq!(after(&a, "-frames:v"), Some("240"));
    assert_eq!(after(&a, "-c:v"), Some("copy"));
    assert_eq!(after(&a, "-output_ts_offset"), Some("1.666667"));
    assert_eq!(a.last().map(String::as_str), Some("o.ts"));
}

#[test]
fn encode_piece_matches_the_source() {
    let p = Piece {
        start: 100,
        end: 120,
        at: 0,
        copy: false,
    };
    let a = args(Path::new("in.mp4"), &[p], &info(), true, Path::new("o.ts"));
    assert_eq!(after(&a, "-ss"), Some("3.341667"));
    assert_eq!(after(&a, "-c:v"), Some("h264_nvenc"));
    assert_eq!(after(&a, "-profile:v"), Some("main"));
    assert_eq!(after(&a, "-level"), Some("4.0"));
    assert_eq!(after(&a, "-colorspace"), Some("bt709"));
    assert_eq!(
        after(&a, "-color_trc"),
        None,
        "unknown tags are not invented"
    );
    let full = args(Path::new("in.mp4"), &[p], &info(), false, Path::new("o.ts"));
    assert_eq!(after(&a, "-hwaccel"), Some("cuda"));
    assert_eq!(after(&full, "-level"), None);
    assert_eq!(after(&full, "-hwaccel"), None);
    assert_eq!(after(&full, "-pix_fmt"), Some("yuv420p"));
}

#[test]
fn a_batch_selects_its_frames_and_numbers_them_back_to_back() {
    let ps = [
        Piece {
            start: 100,
            end: 120,
            at: 50,
            copy: false,
        },
        Piece {
            start: 150,
            end: 160,
            at: 70,
            copy: false,
        },
    ];
    let a = args(Path::new("in.mp4"), &ps, &info(), true, Path::new("o.ts"));
    assert_eq!(after(&a, "-ss"), Some("3.341667"));
    assert_eq!(
        after(&a, "-vf"),
        Some("select='between(n,0,19)+between(n,50,59)',setpts=N*1/30/TB")
    );
    assert_eq!(after(&a, "-frames:v"), Some("30"));
    assert_eq!(after(&a, "-output_ts_offset"), Some("2.666667"));
    // A single piece has no filter.
    assert_eq!(
        after(
            &args(Path::new("i"), &ps[..1], &info(), true, Path::new("o")),
            "-vf"
        ),
        None
    );
}
