//! The layout on a tiny word-level vocabulary (the real one is pinned against the SDK by the ignored
//! golden test in `tests/models.rs`), and the batching without a model.

use super::*;

/// Words split on spaces and punctuation; ids 0-4 as Laya's special tokens are, unknown words `<unk>`.
fn vocab() -> Vocab {
    let words = [
        "<pad>", "<eos>", "<bos>", "<unk>", "<mask>", "choice", "question", ":", "keep", "cut",
        "it", "stays", "goes", "a", "b", "is", "?",
    ];
    let map: Vec<String> = words
        .iter()
        .enumerate()
        .map(|(i, w)| format!("{}: {i}", serde_json::to_string(w).unwrap()))
        .collect();
    let json = format!(
        r#"{{"version":"1.0","truncation":null,"padding":null,"added_tokens":[],"normalizer":null,
        "pre_tokenizer":{{"type":"Whitespace"}},"post_processor":null,"decoder":null,
        "model":{{"type":"WordLevel","vocab":{{{}}},"unk_token":"<unk>"}}}}"#,
        map.join(",")
    );
    Vocab::read(&json).unwrap()
}

fn q(instructions: &str) -> Question {
    Question {
        instructions: instructions.into(),
        keep: "it stays".into(),
        cut: "it goes".into(),
    }
}

#[test]
fn the_question_and_its_options_are_laid_out_as_the_sdk_lays_them() {
    let v = vocab();
    let head = v.head(&q("is it a ?")).unwrap();
    // <bos> choice question : is it a ? <eos> <mask> keep : it stays <mask> cut : it goes <eos>
    assert_eq!(
        head.0,
        [2, 5, 6, 7, 15, 10, 13, 16, 1, 4, 8, 7, 10, 11, 4, 9, 7, 10, 12, 1]
    );
    assert_eq!(head.1, [9, 14]);
    // A state with a <mask> of its own reads it as a space: no third marker.
    let row = v.sequence(&head, "a <mask> b").unwrap();
    assert_eq!(&row.ids[head.0.len()..], [13, 14, 1]);
    assert_eq!(row.markers, [9, 14]);
}

#[test]
fn a_long_state_is_cut_at_its_end_and_long_instructions_to_the_heads_budget() {
    let v = vocab();
    let head = v.head(&q("a")).unwrap();
    let long = vec!["b"; 2000].join(" ");
    assert!(!v.fits(&head, &long).unwrap());
    assert!(v.fits(&head, "a b").unwrap());
    let row = v.sequence(&head, &long).unwrap();
    assert_eq!(row.ids.len(), MAX_LEN);
    assert_eq!(*row.ids.last().unwrap(), 1);
    // Exactly the room left, then <eos>.
    let room = MAX_LEN - head.0.len() - 1;
    assert_eq!(
        v.sequence(&head, &vec!["b"; room].join(" "))
            .unwrap()
            .ids
            .len(),
        MAX_LEN
    );
    assert!(v.fits(&head, &vec!["b"; room].join(" ")).unwrap());
    assert!(!v.fits(&head, &vec!["b"; room + 1].join(" ")).unwrap());
    // Instructions past the budget the options leave are cut: 256 - 2 options of 5 tokens.
    let wordy = v.head(&q(&vec!["a"; 400].join(" "))).unwrap();
    assert_eq!(wordy.0.len(), 1 + (HEAD_MAX_LEN - 10) + 1 + 10 + 1);
    assert_eq!(wordy.1, [HEAD_MAX_LEN - 10 + 2, HEAD_MAX_LEN - 10 + 7]);
}

#[test]
fn batches_run_shortest_first_and_answers_come_back_in_order() {
    let row = |n: usize| Row {
        ids: vec![0; n],
        markers: vec![0, 1],
    };
    // Lengths 900, 10, 500 and 40 short ones: the short ones share batches, the long ones fill theirs.
    let mut rows = vec![row(900), row(10), row(500)];
    rows.extend((0..40).map(|i| row(20 + i)));
    let mut sizes = Vec::new();
    let mut told = Vec::new();
    let out = in_batches(
        &rows,
        &mut |b| {
            sizes.push(b.len());
            assert!(b.len() <= BATCH_ROWS);
            assert!(b.iter().map(|r| r.ids.len()).max().unwrap() * b.len() <= BATCH_TOKENS);
            Ok(b.iter().map(|r| r.ids.len() as f32).collect())
        },
        &mut |n| told.push(n),
    )
    .unwrap();
    let want: Vec<f32> = rows.iter().map(|r| r.ids.len() as f32).collect();
    assert_eq!(out, want);
    assert_eq!(sizes.iter().sum::<usize>(), rows.len());
    assert_eq!(*told.last().unwrap(), rows.len());
    // A runner that answers short is an error, not a misplaced answer.
    assert!(in_batches(&rows, &mut |_| Ok(vec![0.0]), &mut |_| {}).is_err());
    assert!(in_batches(&[], &mut |_| unreachable!(), &mut |_| {})
        .unwrap()
        .is_empty());
}
