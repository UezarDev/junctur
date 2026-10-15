//! Embedded video on Windows: mpv's render API drawing through OpenGL onto a native child window
//! under the transparent WebView. A render thread owns both contexts, as render.h asks; the UI
//! thread only creates and resizes the surface window.

mod gl;
mod surface;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::lock::lock;
use crate::mpv::{Mpv, RenderContext};
use surface::Surface;

#[derive(Default)]
struct Wake {
    /// mpv asked for an update (maybe a new frame).
    update: bool,
    /// The surface changed size or needs repainting: draw even if mpv has no new frame. Pokes
    /// that arrive while a frame is being drawn merge into one flag, so a resize drag costs one
    /// redraw per render-loop turn, not one per resize event.
    redraw: bool,
    size: (i32, i32),
    quit: bool,
}

#[derive(Default)]
struct Shared {
    wake: Mutex<Wake>,
    cv: Condvar,
    /// New mpv frames swapped onto the surface (not redraws), for test mode.
    frames: AtomicU64,
}

impl Shared {
    fn poke(&self, f: impl FnOnce(&mut Wake)) {
        f(&mut lock(&self.wake));
        self.cv.notify_one();
    }
}

pub struct VideoOutput {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    surface: Surface,
}

impl VideoOutput {
    /// Puts a surface inside the window `parent` (client size `w`x`h`, physical pixels) and starts
    /// rendering `mpv` onto it. Call on the UI thread, before any file is loaded. Returns once
    /// mpv's render context exists, so a `loadfile` right after finds its video output.
    pub fn attach(parent: isize, w: u32, h: u32, mpv: Arc<Mpv>) -> Result<Self, String> {
        let (w, h) = (w as i32, h as i32);
        let shared = Arc::new(Shared::default());
        let sh = shared.clone();
        let surface = Surface::create(parent, w, h, move || sh.poke(|s| s.redraw = true))?;
        // First draw right away: the surface shows garbage until something is rendered.
        shared.poke(|s| {
            s.size = (w, h);
            s.redraw = true;
        });
        let (ready_tx, ready_rx) = mpsc::channel();
        let hwnd = surface.hwnd();
        let sh = shared.clone();
        let thread = std::thread::Builder::new()
            .name("video".into())
            .spawn(move || render_thread(hwnd, mpv, sh, ready_tx))
            .map_err(|e| e.to_string())?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                shared,
                thread: Some(thread),
                surface,
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("the video thread stopped while starting".into()),
        }
    }

    /// Follows the window's client area (resize, DPI change, minimise as 0x0).
    pub fn resize(&self, w: u32, h: u32) {
        let (w, h) = (w as i32, h as i32);
        self.surface.fit(w, h);
        self.shared.poke(|s| {
            s.size = (w, h);
            s.redraw = true;
        });
    }

    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    pub fn frames(&self) -> u64 {
        self.shared.frames.load(Ordering::Relaxed)
    }

    /// Stops the render thread, which frees mpv's render context before mpv itself can go.
    pub fn stop(&mut self) {
        self.shared.poke(|s| s.quit = true);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for VideoOutput {
    fn drop(&mut self) {
        self.stop();
    }
}

fn render_thread(
    hwnd: isize,
    mpv: Arc<Mpv>,
    shared: Arc<Shared>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let setup = || -> Result<(gl::Gl, RenderContext), String> {
        let gl = gl::Gl::new(hwnd)?;
        let sh = shared.clone();
        let rc = RenderContext::opengl(mpv, gl::get_proc_address, gl.proc_ctx(), move || {
            sh.poke(|s| s.update = true)
        })?;
        Ok((gl, rc))
    };
    let (gl, rc) = match setup() {
        Ok(v) => {
            let _ = ready.send(Ok(()));
            v
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let mut errors = ErrorLog::default();
    loop {
        let (update, redraw, (w, h)) = {
            let mut s = lock(&shared.wake);
            while !s.update && !s.redraw && !s.quit {
                s = shared.cv.wait(s).unwrap_or_else(|e| e.into_inner());
            }
            if s.quit {
                break;
            }
            let v = (s.update, s.redraw, s.size);
            s.update = false;
            s.redraw = false;
            v
        };
        // mpv_render_context_update must follow every update callback, frame or not.
        let frame = update && rc.needs_frame();
        if (frame || redraw) && w > 0 && h > 0 {
            if let Err(e) = rc.render(0, w, h) {
                errors.report(&e);
                continue;
            }
            gl.swap();
            rc.report_swap();
            if frame {
                shared.frames.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    // The render context goes first, while its GL context is still current.
    drop(rc);
    drop(gl);
}

/// Render errors repeat on every update while they last; log the first, then at most one line
/// (with the count) per interval.
#[derive(Default)]
struct ErrorLog {
    last: Option<Instant>,
    skipped: u64,
}

impl ErrorLog {
    const EVERY: Duration = Duration::from_secs(5);

    fn report(&mut self, e: &str) {
        if self.last.is_some_and(|t| t.elapsed() < Self::EVERY) {
            self.skipped += 1;
            return;
        }
        match self.skipped {
            0 => eprintln!("junctur-app: {e}"),
            n => eprintln!("junctur-app: {e} ({n} more since the last report)"),
        }
        self.last = Some(Instant::now());
        self.skipped = 0;
    }
}
