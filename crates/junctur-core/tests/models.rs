//! The ONNX models end to end. Ignored by default because the first run downloads ~100 MB into the
//! user cache; run with `cargo test -- --ignored`.

mod common;

use common::{ffmpeg, workdir};
use std::path::Path;

use junctur_core::diarize::{segment, Embedder, Segmenter};
use junctur_core::ffmpeg::{PcmReader, Span};
use junctur_core::runtime::DevicePref;
use junctur_core::tracks::Role;
use junctur_core::vad::{self, Vad};
use junctur_core::{analyze, cache, models, separate, AnalyzeOptions, SeparateOptions};

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
    let mut s = Segmenter::open(&m).unwrap();
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
    let d = workdir("models", "tracks");
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
    let d = workdir("models", "sep");
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
    let mut e = Embedder::open(&m).unwrap();
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
    // Speed on caption-length sentences, printed.
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
    let d = workdir("models", "translate");
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

/// The word list reaches Whisper as prompt tokens, a long list cut to the prompt's room from
/// the end, and a chunk decodes with it (no panic on the tokenizer's count, no failure). Turbo, on
/// the CPU: about 1.6 GB downloaded on a first run.
#[test]
#[ignore = "downloads models"]
fn whisper_takes_the_word_list_and_cuts_a_long_one() {
    use junctur_core::runtime::Device;
    use junctur_core::transcribe::Whisper;
    let m = models::ensure(&models::WHISPER_LARGE_V3_TURBO, &cache::default_dir()).unwrap();
    let mut w = Whisper::open(&m, Device::Cpu).unwrap();
    let long: Vec<String> = (0..400).map(|i| format!("Tsunheto{i}")).collect();
    let p = w.set_prompt(&long);
    assert!(p.kept > 5 && p.kept < 400, "{}", p.kept);
    assert_eq!(p.left_out.first(), Some(&format!("Tsunheto{}", p.kept)));
    let p = w.set_prompt(&["Uezar".to_string(), "Delay".into()]);
    assert_eq!((p.text.as_str(), p.kept), ("Uezar, Delay", 2));
    // Two seconds of a quiet tone: whatever it hears, it decodes with the prompt set.
    let x: Vec<f32> = (0..32_000)
        .map(|i| 0.01 * (i as f32 * 0.05).sin())
        .collect();
    w.transcribe(&x, "en").unwrap();
    assert_eq!(w.set_prompt(&[]).kept, 0);
}

/// SigLIP 2 reads a test card as one, the same on every device it opens on (CAM++ on CUDA did not,
/// phase 5r), and a frame embeds the same alone and in a batch. The text tower runs once: a second
/// ask reads the kept embeddings.
#[test]
#[ignore = "downloads models"]
fn siglip_reads_a_test_card_alike_on_every_device() {
    use junctur_core::labels::model::{self, Vision, SIZE};
    let cache = cache::default_dir();
    let d = workdir("models", "siglip");
    let raw = d.join("bars.rgb");
    let size = format!("smptebars=size={SIZE}x{SIZE}:duration=1");
    let raw_s = raw.to_string_lossy().into_owned();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &size,
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        &raw_s,
    ]);
    let px = std::fs::read(&raw).unwrap();
    let mut cpu = Vision::open(&cache, DevicePref::Cpu).unwrap();
    let mut any = Vision::open(&cache, DevicePref::Auto).unwrap();
    let a = cpu.embed(&[&px]).unwrap().remove(0);
    let b = any.embed(&[&px, &px]).unwrap();
    let dot = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(p, q)| p * q).sum::<f32>();
    eprintln!(
        "SigLIP on {:?}: {:.5} against the CPU",
        any.device,
        dot(&a, &b[0])
    );
    assert!(
        dot(&a, &b[0]) > 0.99,
        "{:?} differs from the CPU",
        any.device
    );
    assert!(dot(&b[0], &b[1]) > 0.9999);
    let prompts: Vec<String> = ["colour bars test pattern", "a video game fight", "a cat"]
        .map(String::from)
        .to_vec();
    let t = model::texts(&cache, &prompts).unwrap();
    let s = model::shares(&a, &t);
    eprintln!("shares {s:?}");
    assert!(s[0] > s[1] && s[0] > s[2], "{s:?}");
    assert_eq!(model::texts(&cache, &prompts).unwrap(), t);
    let _ = std::fs::remove_dir_all(&d);
}

/// Laya reads a state as its Python SDK does: the same token ids and option markers for the six
/// states of `data/laya_golden.json` (a dict state spelled as `json.dumps` spells it, a `<mask>` in
/// the text, a state past 1024 tokens), and P(keep) within 0.01 of the SDK's PyTorch model, on the
/// CPU and on the best device this build has (the fp16 export was within 0.0045 in Python).
#[test]
#[ignore = "downloads models"]
fn laya_reads_states_as_the_python_sdk_does() {
    use junctur_core::keep::laya::{self, Laya, Question, Vocab};
    use junctur_core::keep::state::dumps;
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("data/laya_golden.json")).unwrap();
    let cache = cache::default_dir();
    let q = &golden["question"];
    let q = Question {
        instructions: q["instructions"].as_str().unwrap().into(),
        keep: q["keep"].as_str().unwrap().into(),
        cut: q["cut"].as_str().unwrap().into(),
    };
    assert_eq!(golden["max_len"], laya::MAX_LEN);
    assert_eq!(golden["head_max_len"], laya::HEAD_MAX_LEN);
    let vocab = Vocab::open(&cache).unwrap();
    let head = vocab.head(&q).unwrap();
    let mut rows = Vec::new();
    let mut want = Vec::new();
    for (i, c) in golden["cases"].as_array().unwrap().iter().enumerate() {
        let text = c["text"].as_str().unwrap();
        if let Some(o) = c["state"].as_object() {
            // In the state's own order (serde_json's map is sorted).
            let members: Vec<(&str, &str)> = ["video", "before", "this part", "after"]
                .into_iter()
                .map(|k| (k, o[k].as_str().unwrap()))
                .collect();
            assert_eq!(dumps(&members), text, "case {i} spelled otherwise");
        }
        let row = vocab.sequence(&head, text).unwrap();
        let ids: Vec<u32> = serde_json::from_value(c["ids"].clone()).unwrap();
        let markers: Vec<usize> = serde_json::from_value(c["markers"].clone()).unwrap();
        assert_eq!(row.ids, ids, "case {i}: other ids");
        assert_eq!(row.markers, markers, "case {i}: other markers");
        rows.push(row);
        want.push(c["p_keep"].as_f64().unwrap());
    }
    for pref in [DevicePref::Cpu, DevicePref::Auto] {
        let mut model = Laya::open(&cache, &vocab, pref).unwrap();
        let margins = model.margins(&rows, &mut |_| {}).unwrap();
        for (i, (m, w)) in margins.iter().zip(&want).enumerate() {
            let p = laya::p_keep(*m as f64, 1.0);
            eprintln!("Laya on {:?}, case {i}: {p:.4} (SDK {w:.4})", model.device);
            assert!(
                (p - w).abs() < 0.01,
                "case {i} on {:?}: {p} against {w}",
                model.device
            );
        }
    }
}
