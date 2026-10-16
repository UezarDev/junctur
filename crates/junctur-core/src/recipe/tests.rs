use super::sets::{self, Spans};
use super::*;

/// Detectors over made-up signals: speech at 0-10 and 20-30, a still picture at 5-25, Ana
/// talking 20-30, "food" said at 22, examples matching at 40, 41 and 50.
struct Fake;

impl Signals for Fake {
    fn silence(&mut self, r: (f64, f64)) -> Result<Spans> {
        Ok(sets::not(&[(0.0, 10.0), (20.0, 30.0)], r))
    }
    fn still(&mut self, _: (f64, f64)) -> Result<Spans> {
        Ok(vec![(5.0, 25.0)])
    }
    fn speaker(&mut self, name: &str, _: (f64, f64)) -> Result<Spans> {
        match name {
            "Ana" => Ok(vec![(20.0, 30.0)]),
            ANYONE => Ok(vec![(0.0, 10.0), (20.0, 30.0)]),
            n => Err(Error::Recipe(format!("nobody is called {n}"))),
        }
    }
    fn words(&mut self, _: &str, remark: bool, _: (f64, f64)) -> Result<Spans> {
        Ok(if remark {
            vec![(21.0, 25.0)]
        } else {
            vec![(22.0, 22.4)]
        })
    }
    fn like_this(&mut self, _: (f64, f64)) -> Result<Spans> {
        Ok(vec![(40.0, 40.25), (41.0, 41.25), (50.0, 50.25)])
    }
    fn label(&mut self, _: &str, _: (f64, f64)) -> Result<Spans> {
        Ok(vec![(60.0, 70.0)])
    }
}

fn run(json: &str, range: (f64, f64)) -> Plan {
    plan(&parse(json).unwrap(), range, &mut Fake).unwrap()
}

#[test]
fn silence_and_no_movement_cuts_where_both_hold() {
    let p = run(
        r#"{"action":"cut_where","when":{"all":[{"silence":true},{"still_picture":true}]}}"#,
        (0.0, 100.0),
    );
    assert_eq!(p.cuts, [(10.0, 20.0)]);
    assert!(p.keeps.is_empty());
    // Only inside the selection.
    let p = run(
        r#"{"action":"cut_where","when":{"all":[{"silence":true},{"still_picture":true}]}}"#,
        (15.0, 18.0),
    );
    assert_eq!(p.cuts, [(15.0, 18.0)]);
    // A part under min_seconds is left alone.
    let p = run(
        r#"{"action":"cut_where","when":{"silence":true},"min_seconds":15}"#,
        (0.0, 100.0),
    );
    assert_eq!(p.cuts, [(30.0, 100.0)]);
}

#[test]
fn keep_around_each_event_cuts_the_rest_of_the_range() {
    let p = run(
        r#"{"action":"keep_around","when":{"like_this":"ore"},"before":0.1,"after":0.15}"#,
        (35.0, 55.0),
    );
    assert_eq!(p.keeps, [(39.9, 40.15), (40.9, 41.15), (49.9, 50.15)]);
    assert_eq!(
        p.cuts,
        [(35.0, 39.9), (40.15, 40.9), (41.15, 49.9), (50.15, 55.0)]
    );
    assert_eq!(p.found, 3);
    // Defaults are the quarter second.
    let r = parse(r#"{"action":"cut_around","when":{"like_this":"x"}}"#).unwrap();
    assert_eq!(r.around(), AROUND);
}

#[test]
fn what_a_speaker_says_about_a_phrase_is_the_whole_remark() {
    let p = run(
        r#"{"action":"cut_where","when":{"all":[{"speaker":"Ana"},{"words":"food"}]},"min_seconds":0}"#,
        (0.0, 100.0),
    );
    assert_eq!(p.cuts, [(21.0, 25.0)]);
    // As an event, the saying itself.
    let p = run(r#"{"action":"mark","when":{"words":"food"}}"#, (0.0, 100.0));
    assert_eq!(p.marks, [22.0]);
    let p = run(
        r#"{"action":"clip_each","when":{"words":"food"},"before":1,"after":1}"#,
        (0.0, 100.0),
    );
    assert_eq!(p.clips, [(21.0, 23.4)]);
}

#[test]
fn keep_where_not_and_any() {
    let p = run(
        r#"{"action":"keep_where","when":{"not":{"silence":true}}}"#,
        (0.0, 40.0),
    );
    assert_eq!(p.keeps, [(0.0, 10.0), (20.0, 30.0)]);
    assert_eq!(p.cuts, [(10.0, 20.0), (30.0, 40.0)]);
    let p = run(
        r#"{"action":"cut_where","when":{"any":[{"label":"menu"},{"speaker":"Ana"}]}}"#,
        (0.0, 100.0),
    );
    assert_eq!(p.cuts, [(20.0, 30.0), (60.0, 70.0)]);
}

#[test]
fn a_tool_that_cannot_answer_is_said() {
    let r = parse(r#"{"action":"cut_where","when":{"speaker":"Bob"}}"#).unwrap();
    let e = plan(&r, (0.0, 1.0), &mut Fake).unwrap_err();
    assert!(e.to_string().contains("Bob"), "{e}");
}

#[test]
fn only_what_is_on_the_menu_is_read() {
    for bad in [
        "",
        "no json here",
        r#"{"action":"explode","when":{"silence":true}}"#,
        r#"{"action":"cut_where"}"#,
        r#"{"action":"cut_where","when":{"music":true}}"#,
        r#"{"action":"cut_where","when":{"silence":false}}"#,
        r#"{"action":"cut_where","when":{"silence":true,"music":true}}"#,
        r#"{"action":"cut_where","when":{}}"#,
        r#"{"action":"cut_where","when":{"speaker":""}}"#,
        r#"{"action":"cut_where","when":{"all":[]}}"#,
        r#"{"action":"cut_where","when":{"silence":true},"why":"because"}"#,
        r#"{"action":"cut_where","when":{"silence":true},"min_seconds":-1}"#,
        r#"{"action":"cut_where","when":{"silence":true},"before":"two"}"#,
        r#"{"action":"cut_where","when":{"not":{"not":{"not":{"not":{"silence":true}}}}}}"#,
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn several_tools_in_one_condition_are_all_of_them() {
    let r =
        parse(r#"{"action":"cut_where","when":{"silence":true,"still_picture":true}}"#).unwrap();
    assert_eq!(r.when, Cond::All(vec![Cond::Silence, Cond::StillPicture]));
}

#[test]
fn the_first_object_is_read_whatever_is_around_it() {
    let r = parse(
        "Sure!\n```json\n{\"action\": \"cut_where\", \"when\": {\"words\": \"a } b\"}}}\n```",
    )
    .unwrap();
    assert_eq!(r.when, Cond::Words("a } b".into()));
    // And back.
    assert_eq!(from_json(&r.to_json()).unwrap(), r);
}

#[test]
fn plain_words() {
    let r =
        parse(r#"{"action":"cut_where","when":{"all":[{"silence":true},{"still_picture":true}]}}"#)
            .unwrap();
    assert_eq!(
        r.describe(),
        "Cut where: nobody talks AND the picture is still, longer than 1 s"
    );
    let r = parse(r#"{"action":"keep_around","when":{"any":[{"like_this":"ore"},{"not":{"silence":true}}]},"before":0.1,"after":0.15}"#).unwrap();
    assert_eq!(
        r.describe(),
        "Keep 0.1 s before to 0.15 s after each time: a sound like the marked one (ore) OR NOT nobody talks; cut the rest"
    );
}

fn keys(request: &str) -> Recipe {
    keywords::read(request, &["Tsunheto".to_string(), "Ana".to_string()]).unwrap()
}

#[test]
fn the_keyword_reader_reads_the_research_examples() {
    let r = keys("remove parts with silence and no movement");
    assert_eq!(r.action, Action::CutWhere);
    assert_eq!(r.when, Cond::All(vec![Cond::Silence, Cond::StillPicture]));
    let r = keys("keep only 1/4 second each time I break an ore, wood, or hit an enemy");
    assert_eq!(r.action, Action::KeepAround);
    assert!(matches!(r.when, Cond::LikeThis(_)));
    assert_eq!((r.before, r.after), (Some(0.1), Some(0.15)));
    let r = keys("cut everything Tsunheto says about food");
    assert_eq!(r.action, Action::CutWhere);
    assert_eq!(
        r.when,
        Cond::All(vec![
            Cond::Speaker("Tsunheto".into()),
            Cond::Words("food".into())
        ])
    );
    let r = keys("cut when the inventory is open");
    assert_eq!(r.when, Cond::Label("an open inventory".into()));
}

#[test]
fn the_keyword_reader_reads_spanish_and_numbers_and_refuses_the_rest() {
    let r = keys("quita las partes donde nadie habla por más de 5 segundos");
    assert_eq!(
        (r.action, &r.when, r.min_seconds),
        (Action::CutWhere, &Cond::Silence, Some(5.0))
    );
    let r = keys("deja medio segundo cada vez que rompo un mineral");
    assert_eq!(r.action, Action::KeepAround);
    assert_eq!((r.before, r.after), (Some(0.2), Some(0.3)));
    let r = keys("only keep what Ana says");
    assert_eq!(
        (r.action, &r.when),
        (Action::KeepWhere, &Cond::Speaker("Ana".into()))
    );
    let r = keys("mark every time someone says \"clip that\"");
    assert_eq!(
        (r.action, &r.when),
        (Action::Mark, &Cond::Words("clip that".into()))
    );
    assert_eq!(keywords::seconds("keep 2.5s"), Some(2.5));
    assert_eq!(keywords::seconds("dos segundos"), Some(2.0));
    assert_eq!(keywords::seconds("level 3"), None);
    assert!(keywords::read("make it funnier", &[]).is_err());
    let r = keys("cut when I'm quiet and don't talk");
    assert_eq!(r.when, Cond::Silence);
}

#[test]
fn set_operations() {
    let x = vec![(0.0, 2.0), (4.0, 6.0)];
    let y = vec![(1.0, 5.0)];
    assert_eq!(sets::and(&x, &y), [(1.0, 2.0), (4.0, 5.0)]);
    assert_eq!(sets::or(&x, &y), [(0.0, 6.0)]);
    assert_eq!(sets::not(&x, (0.0, 10.0)), [(2.0, 4.0), (6.0, 10.0)]);
    assert_eq!(sets::not(&[], (1.0, 2.0)), [(1.0, 2.0)]);
    assert_eq!(
        sets::not(&[(0.0, 5.0)], (1.0, 2.0)),
        Vec::<(f64, f64)>::new()
    );
    assert_eq!(
        sets::around(&[(1.0, 9.0), (1.2, 1.3)], 0.5, 0.5, (0.0, 10.0)),
        [(0.5, 1.7)]
    );
}
