//! The ONNX models end to end. Ignored by default because the first run downloads ~100 MB into the
//! user cache; run with `cargo test -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

use junctur_core::diarize::{segment, Embedder, Segmenter};
use junctur_core::ffmpeg::{PcmReader, Span};
use junctur_core::runtime::DevicePref;
use junctur_core::tracks::Role;
use junctur_core::vad::{self, Vad};
use junctur_core::{analyze, cache, models, separate, AnalyzeOptions, SeparateOptions};

fn workdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("junctur-models-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ffmpeg(args: &[&str]) {
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y"])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "{args:?}");
}

fn read_all(f: &Path, rate: u32, ch: u32) -> Vec<f32> {
    let mut r = PcmReader::open(f, 0, rate, ch, Span::default()).unwrap();
    let mut out = Vec::new();
    loop {
        let x = r.read(1 << 16).unwrap();
        if x.is_empty() {
            break;
        }
        out.extend(x);
    }
    r.finish().unwrap();
    out
}

#[test]
#[ignore = "downloads models"]
fn vad_hears_no_speech_in_tones_or_silence() {
    let m = models::ensure(&models::SILERO_VAD, &cache::default_dir()).unwrap();
    let mut v = Vad::open(&m).unwrap();
    let tone: Vec<f32> = (0..48_000)
        .map(|i| 0.3 * (i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin())
        .collect();
    let p = v.probs(&tone).unwrap();
    assert_eq!(p.len(), 48_000_usize.div_ceil(vad::FRAME));
    assert!(vad::regions(&p, 0.0).is_empty());
    v.reset();
    assert!(v.probs(&[0.0; 16_000]).unwrap().iter().all(|&x| x < 0.5));
}

#[test]
#[ignore = "downloads models"]
fn segmentation_hears_nobody_in_silence_or_a_tone() {
    let m = models::ensure(&models::SEGMENTATION, &cache::default_dir()).unwrap();
    let mut s = Segmenter::open(&m, DevicePref::Cpu).unwrap();
    let n = segment::window_samples();
    let tone: Vec<f32> = (0..n)
        .map(|i| 0.3 * (i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin())
        .collect();
    // A short clip is padded to the window.
    let c = s.classes(&[vec![0.0; n], tone, vec![0.0; 100]]).unwrap();
    assert_eq!(c.len(), 3);
    assert!(
        c.iter().all(|w| w.len() == 589),
        "{:?}",
        c.iter().map(Vec::len).collect::<Vec<_>>()
    );
    for w in &c {
        assert!(segment::overlaps(1, w).is_empty());
    }
    assert!(c[0].iter().all(|&x| x == 0), "silence is nobody");
    // A steady tone is no pair of voices, and mostly nobody.
    assert!(c[1].iter().all(|&x| x < 4), "{:?}", c[1]);
    let quiet = c[1].iter().filter(|&&x| x == 0).count();
    assert!(quiet * 10 >= c[1].len() * 9, "{quiet} of {}", c[1].len());
}

#[test]
#[ignore = "downloads models"]
fn tracks_with_speech_detection_keeps_tone_layouts_sane() {
    let d = workdir("tracks");
    let f = d.join("tone.mp4");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=440:d=40",
        "-c:a",
        "aac",
        f.to_str().unwrap(),
    ]);
    let o = AnalyzeOptions {
        windows: 4,
        window_seconds: 3.0,
        use_cache: false,
        ..Default::default()
    };
    let r = analyze(&f, &o).unwrap();
    assert_eq!(r.speech_error, None);
    let s = r.logical[0].speech.unwrap();
    assert!(s.share < 0.05, "{s:?}");
    // A steady tone with no speech is background sound, not a mix of voices.
    assert_eq!(r.logical[0].role, Role::GamePc);
    std::fs::remove_dir_all(d).ok();
}

#[test]
#[ignore = "downloads models"]
fn separation_streams_and_stems_add_up_to_the_mix() {
    let d = workdir("sep");
    let f = d.join("mix.mkv");
    // 14 s: longer than one model chunk (5.9 s), so chunk joins and the tail are exercised.
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=220:d=14",
        "-f",
        "lavfi",
        "-i",
        "anoisesrc=d=14:a=0.05",
        "-filter_complex",
        "[0][1]amix=inputs=2,aformat=channel_layouts=stereo",
        "-ar",
        "44100",
        "-c:a",
        "pcm_f32le",
        f.to_str().unwrap(),
    ]);
    let o = SeparateOptions {
        cache_dir: Some(d.join("cache")),
        ..Default::default()
    };
    // Models come from the shared cache, not the per-test one.
    std::fs::create_dir_all(d.join("cache")).unwrap();
    for m in [
        &models::MDX_VOCALS,
        &models::SILERO_VAD,
        &models::SPEAKER_EMBEDDING,
    ] {
        let src = models::ensure(m, &cache::default_dir()).unwrap();
        let dst = models::dir(&d.join("cache"));
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::copy(&src, dst.join(m.file)).unwrap();
    }
    let s = separate(&f, 0, &o).unwrap();
    assert!((s.seconds - 14.0).abs() < 0.05, "{}", s.seconds);
    let mix = read_all(&f, 44_100, 2);
    let voc = read_all(&s.vocals, 44_100, 2);
    let bg = read_all(&s.background, 44_100, 2);
    assert_eq!(voc.len(), mix.len());
    assert_eq!(bg.len(), mix.len());
    // FLAC is 24/16-bit, so the sum matches to quantisation, not exactly.
    let err = mix
        .iter()
        .zip(voc.iter().zip(&bg))
        .map(|(m, (v, b))| (m - v - b).abs())
        .fold(0.0, f32::max);
    assert!(err < 1e-3, "{err}");
    // No voice in a tone plus noise: the vocal stem stays well below the mix.
    let e = |x: &[f32]| x.iter().map(|v| v * v).sum::<f32>();
    assert!(e(&voc) < 0.25 * e(&mix), "{} vs {}", e(&voc), e(&mix));
    let again = separate(&f, 0, &o).unwrap();
    assert!(again.from_cache);
    std::fs::remove_dir_all(d).ok();
}

#[test]
#[ignore = "downloads models"]
fn embeddings_are_unit_vectors_and_stable() {
    let m = models::ensure(&models::SPEAKER_EMBEDDING, &cache::default_dir()).unwrap();
    let mut e = Embedder::open(&m, DevicePref::Auto).unwrap();
    let x: Vec<f32> = (0..24_000)
        .map(|i| 0.2 * (i as f32 * 0.07).sin() * (i as f32 * 0.0013).sin())
        .collect();
    let a = e.embed(&x).unwrap().unwrap();
    let b = e.embed(&x).unwrap().unwrap();
    assert_eq!(a.len(), 512);
    let n: f32 = a.iter().map(|v| v * v).sum();
    assert!((n - 1.0).abs() < 1e-3);
    let dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    assert!(dot > 0.999);
    assert!(e.embed(&[0.0; 100]).unwrap().is_none());
}

#[test]
#[ignore = "downloads models"]
fn captions_translate_between_languages() {
    let stop = std::sync::atomic::AtomicBool::new(false);
    let mut m =
        junctur_core::translate::model::Model::open(&cache::default_dir(), DevicePref::Cpu, &stop)
            .unwrap();
    let t0 = std::time::Instant::now();
    let fr = m
        .translate("Hola, ¿qué tal estás hoy?", "es", "fr")
        .unwrap();
    let en = m
        .translate(
            "No sé si vamos a poder terminar el directo a tiempo.",
            "es",
            "en",
        )
        .unwrap();
    eprintln!("{fr} | {en} in {:.2} s", t0.elapsed().as_secs_f64());
    assert!(
        fr.to_lowercase().contains("bonjour") || fr.to_lowercase().contains("salut"),
        "{fr}"
    );
    assert!(en.to_lowercase().contains("know"), "{en}");
    // Speed on caption-length sentences, for the phase notes.
    let lines = [
        "Bueno, vamos a empezar con la partida de hoy.",
        "¿Alguien sabe dónde está el último cofre?",
        "Creo que tenemos que volver al pueblo antes de que se haga de noche.",
        "Gracias a todos por estar aquí otra vez.",
        "Eso fue increíble, no me lo esperaba para nada.",
    ];
    let t0 = std::time::Instant::now();
    for _ in 0..4 {
        for l in lines {
            m.translate(l, "es", "en").unwrap();
        }
    }
    let s = t0.elapsed().as_secs_f64();
    eprintln!(
        "20 sentences one at a time in {s:.2} s ({:.2} s each) on {:?}",
        s / 20.0,
        m.device
    );
    let batch: Vec<(&str, &str)> = lines
        .iter()
        .chain(&lines[..3])
        .map(|l| (*l, "es"))
        .collect();
    let one: Vec<String> = batch
        .iter()
        .map(|(l, f)| m.translate(l, f, "en").unwrap())
        .collect();
    let t0 = std::time::Instant::now();
    let mut together = Vec::new();
    for _ in 0..4 {
        together = m
            .translate_batch(&batch, "en", false)
            .unwrap()
            .into_iter()
            .map(Option::unwrap)
            .collect();
    }
    let s = t0.elapsed().as_secs_f64();
    eprintln!("4 batches of 8 in {s:.2} s ({:.2} s a sentence)", s / 32.0);
    // The 8-bit weights are quantised per input, so a row's numbers shift a little with its
    // batch-mates and a word may differ; the translation is the same sentence.
    let same = together.iter().zip(&one).filter(|(a, b)| a == b).count();
    assert!(
        same >= 6 && together.iter().all(|t| !t.is_empty()),
        "{together:?} / {one:?}"
    );

    // A run keeps its translations in the project folder, each sentence from the language its
    // segment was heard in.
    use junctur_core::transcribe::Segment;
    use junctur_core::translate::{self, Sentence, Translations};
    let d = workdir("translate");
    let seg = |start: f64, lang: &str| Segment {
        track: 0,
        start,
        end: start + 10.0,
        lang: lang.into(),
        model: String::new(),
        words: Vec::new(),
    };
    let sentence = |start: f64, text: &str| Sentence {
        track: 0,
        start,
        text: text.into(),
    };
    let sentences = [sentence(1.0, lines[0]), sentence(12.0, "Thanks, everyone.")];
    let done = translate::run(
        &d,
        &cache::default_dir(),
        "fr",
        &sentences,
        &[seg(0.0, "es"), seg(10.0, "en")],
        &mut |_, _| {},
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!((done.translated, done.kept), (2, 0));
    let t = Translations::load(&d);
    assert!(
        t.get("fr", lines[0]).is_some_and(|f| f != lines[0]),
        "{t:?}"
    );
    eprintln!(
        "{:?} {:?}",
        t.get("fr", lines[0]),
        t.get("fr", "Thanks, everyone.")
    );
    let _ = std::fs::remove_dir_all(&d);
}
