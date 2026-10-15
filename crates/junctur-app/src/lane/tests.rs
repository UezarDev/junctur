use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn wait_for(what: &str, ok: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ok() {
        assert!(Instant::now() < deadline, "never: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn recordings_take_the_lane_in_the_order_they_came() {
    let lane = Arc::new(Lane::default());
    let order = Arc::new(Mutex::new(Vec::new()));
    let waited = Arc::new(AtomicUsize::new(0));
    let first = lane.enter(Path::new("a"), || panic!("a lone recording never waits"));
    let mut threads = Vec::new();
    for name in ["b", "c", "d"] {
        let (mine, order, waited) = (lane.clone(), order.clone(), waited.clone());
        threads.push(std::thread::spawn(move || {
            let _turn = mine.enter(Path::new(name), || {
                waited.fetch_add(1, Ordering::SeqCst);
            });
            order.lock().unwrap().push(name);
        }));
        // Each queued before the next comes.
        let n = threads.len() + 1;
        wait_for("queued", || lane.view().queue.len() == n);
    }
    let names = |v: View| {
        v.queue
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(lane.view()), ["a", "b", "c", "d"]);
    drop(first);
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(*order.lock().unwrap(), ["b", "c", "d"]);
    assert_eq!(waited.load(Ordering::SeqCst), 3);
    assert!(lane.view().queue.is_empty());
}

#[test]
fn a_paused_head_holds_between_chunks_until_resumed() {
    let lane = Arc::new(Lane::default());
    assert!(!lane.pause(true), "nothing on the lane: nothing to pause");
    let turn = lane.enter(Path::new("a"), || {});
    // Not paused: a chunk boundary passes straight through.
    lane.hold(|| panic!("not paused"));
    assert!(lane.pause(true));
    assert!(!lane.pause(true), "already paused");
    assert_eq!((lane.view().paused, lane.view().stopped), (true, false));
    let stops = Arc::new(AtomicUsize::new(0));
    let passed = Arc::new(AtomicUsize::new(0));
    let head = {
        let (lane, stops, passed) = (lane.clone(), stops.clone(), passed.clone());
        std::thread::spawn(move || {
            lane.hold(|| {
                stops.fetch_add(1, Ordering::SeqCst);
            });
            passed.fetch_add(1, Ordering::SeqCst);
        })
    };
    wait_for("stopped", || lane.view().stopped);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(passed.load(Ordering::SeqCst), 0, "held while paused");
    assert!(lane.pause(false));
    head.join().unwrap();
    assert_eq!(stops.load(Ordering::SeqCst), 1);
    assert_eq!((lane.view().paused, lane.view().stopped), (false, false));
    drop(turn);
}

#[test]
fn a_pause_is_forgotten_once_the_lane_is_empty() {
    let lane = Lane::default();
    let turn = lane.enter(Path::new("a"), || {});
    lane.pause(true);
    drop(turn);
    assert!(!lane.view().paused);
    // The next recording is not held.
    let _turn = lane.enter(Path::new("b"), || {});
    lane.hold(|| panic!("a pause from an earlier job"));
}

#[test]
fn a_head_that_finishes_while_paused_hands_on_a_held_lane() {
    let lane = Arc::new(Lane::default());
    let first = lane.enter(Path::new("a"), || {});
    let started = Arc::new(AtomicUsize::new(0));
    let next = {
        let (mine, started) = (lane.clone(), started.clone());
        std::thread::spawn(move || {
            let _turn = mine.enter(Path::new("b"), || {});
            // Before Whisper loads: the first `between`.
            mine.hold(|| {});
            started.fetch_add(1, Ordering::SeqCst);
        })
    };
    wait_for("b queued", || lane.view().queue.len() == 2);
    lane.pause(true);
    // `a` was on its last chunk: it ends without reaching another boundary.
    drop(first);
    wait_for("b held", || lane.view().stopped);
    let v = lane.view();
    assert!(v.paused, "b is still on the lane: the pause stays");
    assert_eq!(v.queue, [PathBuf::from("b")]);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(started.load(Ordering::SeqCst), 0, "b held before Whisper");
    lane.pause(false);
    next.join().unwrap();
    assert_eq!(started.load(Ordering::SeqCst), 1);
}

#[test]
fn resumed_and_paused_again_before_waking_is_still_stopped() {
    let lane = Arc::new(Lane::default());
    let _turn = lane.enter(Path::new("a"), || {});
    lane.pause(true);
    let stops = Arc::new(AtomicUsize::new(0));
    let head = {
        let (mine, stops) = (lane.clone(), stops.clone());
        std::thread::spawn(move || {
            mine.hold(|| {
                stops.fetch_add(1, Ordering::SeqCst);
            })
        })
    };
    wait_for("stopped", || lane.view().stopped);
    {
        // Resume, then pause, with the head unable to take the lock in between.
        let mut s = lane.state();
        s.hold = Hold::Running;
        lane.moved.notify_all();
        s.hold = Hold::Pausing;
    }
    wait_for("stopped again", || lane.view().stopped);
    assert_eq!(
        stops.load(Ordering::SeqCst),
        2,
        "the page is told it stopped again"
    );
    lane.pause(false);
    head.join().unwrap();
}

#[test]
fn a_place_given_up_while_waiting_never_blocks_the_lane() {
    let lane = Arc::new(Lane::default());
    let first = lane.enter(Path::new("a"), || {});
    let mine = lane.clone();
    let panicked = std::thread::spawn(move || {
        let _turn = mine.enter(Path::new("b"), || panic!("on_wait failed"));
    })
    .join();
    assert!(panicked.is_err());
    assert_eq!(lane.view().queue, [PathBuf::from("a")]);
    drop(first);
    // The same recording queued twice waits its own turn.
    let one = lane.enter(Path::new("c"), || {});
    let waited = Arc::new(AtomicUsize::new(0));
    let again = {
        let (mine, waited) = (lane.clone(), waited.clone());
        std::thread::spawn(move || {
            let _turn = mine.enter(Path::new("c"), || {
                waited.fetch_add(1, Ordering::SeqCst);
            });
        })
    };
    wait_for("c queued twice", || lane.view().queue.len() == 2);
    assert_eq!(waited.load(Ordering::SeqCst), 1);
    drop(one);
    again.join().unwrap();
    assert!(lane.view().queue.is_empty());
}
