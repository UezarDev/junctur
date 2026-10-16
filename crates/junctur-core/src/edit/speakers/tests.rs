use super::*;

fn t(start: f64, end: f64, speaker: u32) -> Turn {
    Turn {
        start,
        end,
        speaker,
    }
}

#[test]
fn names_start_numbered_and_survive_a_recut() {
    let s = Speakers::new(vec![t(0.0, 1.0, 1), t(2.0, 3.0, 2)], &[None, None], None);
    assert_eq!(s.name(1), Some("Speaker 1"));
    assert_eq!(s.name(2), Some("Speaker 2"));
    let mut renamed = s.clone();
    assert!(renamed.rename(2, "  Lucía "));
    assert!(renamed.recolor(2, "#123456"));
    assert!(!renamed.recolor(9, "#123456"));
    assert_eq!(s.color(1), Some("#F2B33D"));
    assert!(!renamed.rename(9, "x"));
    // A re-cut finds three speakers now: the rename stays, the new one is numbered.
    let again = Speakers::new(vec![], &[None, None, None], Some(&renamed));
    assert_eq!(again.name(2), Some("Lucía"));
    assert_eq!(again.color(2), Some("#123456"));
    assert_eq!(again.name(3), Some("Speaker 3"));
    // Renumbered (the two swap places): a rename goes with what the speaker says, and so
    // does a colour, but "Speaker N" stays a number and never doubles up.
    let swapped = Speakers::new(
        vec![t(0.0, 1.0, 2), t(2.0, 3.0, 1)],
        &[None, None],
        Some(&renamed),
    );
    assert_eq!(
        (swapped.name(1), swapped.color(1)),
        (Some("Lucía"), Some("#123456"))
    );
    assert_eq!(
        (swapped.name(2), swapped.color(2)),
        (Some("Speaker 2"), Some("#F2B33D"))
    );
}

#[test]
fn close_turns_of_one_speaker_join_and_others_do_not() {
    let s = Speakers::new(
        vec![
            t(5.0, 6.0, 1),
            t(0.0, 1.0, 1),
            t(1.5, 2.0, 1),
            t(2.2, 3.0, 2),
        ],
        &[None, None],
        None,
    );
    assert_eq!(
        s.turns,
        vec![t(0.0, 2.0, 1), t(2.2, 3.0, 2), t(5.0, 6.0, 1)]
    );
}

#[test]
fn words_go_to_the_most_overlapping_then_the_nearest_turn() {
    let s = Speakers::new(
        vec![t(0.0, 10.0, 1), t(9.0, 12.0, 2), t(40.0, 41.0, 1)],
        &[None, None],
        None,
    );
    assert_eq!(s.labeller().of_word(0, 1.0, 1.5), Some(1));
    // 9.2-10.8 overlaps speaker 1 by 0.8 s and speaker 2 by 1.6 s.
    assert_eq!(s.labeller().of_word(0, 9.2, 10.8), Some(2));
    // Between turns: the nearer one, within reach.
    assert_eq!(s.labeller().of_word(0, 13.0, 13.5), Some(2));
    assert_eq!(s.labeller().of_word(0, 35.0, 35.5), Some(1));
    assert_eq!(s.labeller().of_word(0, 25.0, 25.5), None);
    // A zero-length word inside a turn, and one exactly on a turn's edge.
    assert_eq!(s.labeller().of_word(0, 11.5, 11.5), Some(2));
    assert_eq!(s.labeller().of_word(0, 40.0, 40.0), Some(1));
    // Nearest before: the turn reaching furthest, not the last to start.
    let long = Speakers::new(vec![t(0.0, 30.0, 1), t(5.0, 6.0, 2)], &[None, None], None);
    assert_eq!(long.labeller().of_word(0, 31.0, 31.5), Some(1));
    assert_eq!(Speakers::default().labeller().of_word(0, 1.0, 2.0), None);
}

#[test]
fn a_word_where_two_people_talk_at_once_is_both_of_theirs() {
    // Speaker 1 talks 0-10, speaker 2 joins in 4-6 (an overlap turn) and talks 10-12.
    let s = Speakers::new(
        vec![t(0.0, 10.0, 1), t(4.0, 6.0, 2), t(10.0, 12.0, 2)],
        &[None, None],
        None,
    );
    let l = s.labeller();
    assert_eq!(l.speakers_of_word(0, 1.0, 1.4), vec![1]);
    assert_eq!(l.speakers_of_word(0, 4.5, 5.0), vec![2, 1]);
    assert_eq!(l.speakers_of_word(0, 11.0, 11.5), vec![2]);
    // A word straddling a change of speaker: its primary only (the middle is in one turn).
    assert_eq!(l.speakers_of_word(0, 9.7, 10.1), vec![1]);
    assert!(l.speakers_of_word(0, 40.0, 41.0).is_empty());
    // Two of one person's turns around another's overlap turn are still joined.
    let j = Speakers::new(
        vec![t(0.0, 3.0, 1), t(2.5, 3.2, 2), t(3.5, 6.0, 1)],
        &[None, None],
        None,
    );
    assert_eq!(j.turns, vec![t(0.0, 6.0, 1), t(2.5, 3.2, 2)]);
}

#[test]
fn an_enrolled_person_names_their_speaker() {
    let ana = Person {
        name: "Ana".into(),
        color: "#123456".into(),
    };
    let mut before = Speakers::new(vec![], &[None, None], None);
    before.rename(1, "Old");
    before.rename(2, "Kept");
    let s = Speakers::found(
        vec![],
        &[None, None],
        &[Some(ana), None],
        Some(&before),
        &[],
    );
    assert_eq!((s.name(1), s.color(1)), (Some("Ana"), Some("#123456")));
    assert_eq!(s.name(2), Some("Kept"));
    // Turns are held to the millisecond, as the file writes them.
    let r = Speakers::new(vec![t(1.23456, 2.0004, 1)], &[None], None);
    assert_eq!(r.turns, vec![t(1.235, 2.0, 1)]);
    assert!(check_name(" Ana ").is_ok_and(|n| n == "Ana"));
    assert!(check_name("").is_err() && check_name("a\nb").is_err());
    assert!(check_name("track=1").is_err() && check_name(&"x".repeat(61)).is_err());
}

#[test]
fn a_speaker_that_is_a_track_takes_every_word_on_it() {
    // Two tracks talking at once: the overlap test alone would give the word to speaker 2.
    let s = Speakers::new(
        vec![t(0.0, 1.0, 1), t(0.0, 10.0, 2)],
        &[Some(0), Some(3)],
        None,
    );
    assert_eq!(s.labeller().of_word(0, 2.0, 3.0), Some(1));
    assert_eq!(s.labeller().of_word(3, 0.2, 0.6), Some(2));
}

/// The overlaps with each previous speaker's spans built once are the ones the pair loop got
/// rebuilding them for every pair, in the same order.
#[test]
fn overlaps_match_spans_rebuilt_per_pair() {
    let name = |id: u32| SpeakerName {
        id,
        name: format!("S{id}"),
        track: None,
        color: String::new(),
    };
    let previous = Speakers {
        names: (1..=5).map(name).collect(),
        turns: (0..200)
            .map(|k| t(k as f64 * 1.5, k as f64 * 1.5 + 2.0, (k * 7 % 5) as u32 + 1))
            .collect(),
    };
    let turns: Vec<Turn> = (0..150)
        .map(|k| {
            t(
                k as f64 * 2.0 + 0.3,
                k as f64 * 2.0 + 1.9,
                (k * 3 % 4) as u32 + 1,
            )
        })
        .collect();
    let new = [0, 1, 3];
    let open = |j: usize| j != 2;
    let mut want = Vec::new();
    for &i in &new {
        for (j, p) in previous.names.iter().enumerate() {
            if !open(j) {
                continue;
            }
            let o = overlap(&spans(&turns, i as u32 + 1), &spans(&previous.turns, p.id));
            if o > 0.0 {
                want.push((o, i, j));
            }
        }
    }
    let got = overlaps(&turns, &new, &previous, &open);
    assert!(!got.is_empty());
    let bits = |v: &[(f64, usize, usize)]| {
        v.iter()
            .map(|&(o, i, j)| (o.to_bits(), i, j))
            .collect::<Vec<_>>()
    };
    assert_eq!(bits(&got), bits(&want));
}

#[test]
fn a_stretch_given_to_someone_takes_it_from_the_others() {
    let mut s = Speakers::new(
        vec![
            t(0.0, 4.0, 1),
            t(4.5, 5.0, 2),
            t(5.2, 9.0, 1),
            t(10.0, 12.0, 2),
        ],
        &[None, None],
        None,
    );
    // Speaker 1 split around the stretch, speaker 2's own turn there joined into it.
    assert!(s.assign(3.0, 6.0, 2));
    assert_eq!(
        s.turns,
        [
            t(0.0, 3.0, 1),
            t(3.0, 6.0, 2),
            t(6.0, 9.0, 1),
            t(10.0, 12.0, 2)
        ]
    );
    assert!(!s.assign(3.0, 6.0, 9));
    assert!(!s.assign(6.0, 6.0, 1));
    // A new person: the next id, a colour nobody has.
    let id = s.add(" Ana ");
    assert_eq!(id, 3);
    assert_eq!(s.name(3), Some("Ana"));
    let colors: Vec<&str> = s.names.iter().map(|n| n.color.as_str()).collect();
    assert!(!colors[..2].contains(&colors[2]));
    assert!(s.assign(10.5, 11.0, 3));
    assert_eq!(
        s.turns[3..],
        [t(10.0, 10.5, 2), t(10.5, 11.0, 3), t(11.0, 12.0, 2)]
    );
}
