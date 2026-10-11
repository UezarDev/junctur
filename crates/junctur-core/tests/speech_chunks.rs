//! VAD chunking end to end on a generated clip: tone bursts over faint noise stand in for speech,
//! so the test needs ffmpeg but not Whisper.

use std::process::Command;

use junctur_core::transcribe::speech_chunks;

#[test]
fn bursts_become_padded_chunks() {
    let d = std::env::temp_dir().join(format!("junctur-test-{}-vad", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("bursts.m4a");
    // Speech-like bursts at 2-4 s and 7-8 s; a 0.3 s gap inside the first must not split it.
    let expr = "0.3*sin(2*PI*440*t)*(between(t,2,2.8)+between(t,3.1,4)+between(t,7,8))";
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i"])
        .arg(format!("aevalsrc='{expr}':s=48000:d=10"))
        .args(["-f", "lavfi", "-i", "anoisesrc=d=10:a=0.0005:r=48000"])
        .args(["-filter_complex", "[0][1]amix=inputs=2:normalize=0"])
        .args(["-c:a", "aac"])
        .arg(&f)
        .status()
        .expect("ffmpeg on PATH")
        .success();
    assert!(ok);

    let c = speech_chunks(&f, 0, 10.0).unwrap();
    assert_eq!(c.len(), 2, "{c:?}");
    assert!(
        (c[0].start - 1.8).abs() < 0.1 && (c[0].end - 4.2).abs() < 0.1,
        "{c:?}"
    );
    assert!(
        (c[1].start - 6.8).abs() < 0.1 && (c[1].end - 8.2).abs() < 0.1,
        "{c:?}"
    );
    let _ = std::fs::remove_dir_all(&d);
}
