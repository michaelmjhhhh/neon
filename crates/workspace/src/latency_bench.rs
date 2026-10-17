//! Keystroke-to-screen latency benchmark, run inside the real app.
//!
//! `NEON_LATENCY_BENCH=1 neon` opens a large generated document, types into
//! the editor on a timer, and reports how long each keystroke took to reach
//! a presented frame. Keystrokes go through the same dispatch path as
//! AppKit's `insertText:` (key-down dispatch, then the focused input
//! handler), and frames are drawn by the real display link and submitted to
//! Metal, so the numbers include vsync waits and missed frames.
//!
//! Each keystroke is matched to the first frame whose draw started after the
//! keystroke was dispatched; latency runs from the start of dispatch to the
//! end of that frame's submission. The OS input path before dispatch and the
//! display scanout after submission are outside the app and not included.
//!
//! Environment:
//! - `NEON_LATENCY_BENCH=1` runs the benchmark, prints a report and quits.
//! - `NEON_LATENCY_KEYS=N` sets the number of measured keystrokes (default 400).
//! - `NEON_LATENCY_OUT=path` also writes the report as JSON.
//! - `NEON_LATENCY_LABEL=name` names the run in the report.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::component::input::Position;
use gpui_kit::{App, Entity, FrameEvent, FrameTimingCollector, Keystroke, Window, WindowId};

use crate::Workspace;

const WARMUP_KEYSTROKES: usize = 40;
const DEFAULT_KEYSTROKES: usize = 400;
/// Typing cadence: 45-55 ms between keys, about 20 characters per second.
/// The jitter spreads keystrokes across the display's refresh phase.
const BASE_INTERVAL: Duration = Duration::from_millis(45);
const JITTER_MS: u64 = 10;
const TYPED_TEXT: &str = "the quick brown fox jumps over the lazy dog. ";
const FIXTURE_SECTIONS: usize = 300;
/// Give up on keystrokes not typed by then, e.g. while the window stayed in
/// the background, and report what was measured.
const TIME_LIMIT: Duration = Duration::from_secs(90);

/// Benchmark settings read from the environment.
#[derive(Clone, Debug)]
pub struct LatencyBench {
    keystrokes: usize,
    output: Option<PathBuf>,
    label: String,
}

impl LatencyBench {
    /// `Some` when `NEON_LATENCY_BENCH` is set.
    pub fn from_env() -> Option<Self> {
        std::env::var_os("NEON_LATENCY_BENCH")?;
        Some(Self {
            keystrokes: std::env::var("NEON_LATENCY_KEYS")
                .ok()
                .and_then(|keys| keys.parse().ok())
                .unwrap_or(DEFAULT_KEYSTROKES),
            output: std::env::var_os("NEON_LATENCY_OUT").map(PathBuf::from),
            label: std::env::var("NEON_LATENCY_LABEL").unwrap_or_else(|_| "neon".into()),
        })
    }

    /// Write the fixture document and return its path.
    pub fn fixture_path(&self) -> std::io::Result<PathBuf> {
        let path = std::env::temp_dir().join("neon-latency-fixture.md");
        std::fs::write(&path, fixture().0)?;
        Ok(path)
    }

    /// Type into `workspace` once its document has loaded, then print the
    /// report and quit.
    pub fn start(self, workspace: Entity<Workspace>, window: &mut Window, cx: &mut App) {
        let window_id = window.window_handle().window_id();

        window
            .spawn(cx, async move |cx| {
                let wait_until =
                    |cx: &mut gpui_kit::AsyncWindowContext,
                     ready: &dyn Fn(&Workspace, &App) -> bool| {
                        cx.update(|_, cx| ready(workspace.read(cx), cx))
                            .unwrap_or(true)
                    };
                while !wait_until(cx, &|workspace, cx| {
                    !workspace.is_loading() && !workspace.preview().read(cx).is_parsing()
                }) {
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                }

                let cursor_line = fixture().1;
                cx.update(|window, cx| {
                    workspace.update(cx, |workspace, cx| {
                        workspace.editor().update(cx, |editor, cx| {
                            editor.set_cursor_position(
                                Position::new(cursor_line as u32, u32::MAX),
                                window,
                                cx,
                            );
                        });
                    });
                })
                .ok();
                cx.update(|window, _| window.activate_window()).ok();

                // Let the window settle: first layout, focus, font loading.
                cx.background_executor().timer(Duration::from_secs(1)).await;

                let mut rng = 0x2545_f491_4f6c_dd1d_u64;
                let mut typed = TYPED_TEXT.chars().cycle();
                let mut samples = Vec::with_capacity(self.keystrokes);
                let mut collector = None;
                let deadline = Instant::now() + TIME_LIMIT;
                let mut typed_count = 0;
                let mut was_active = true;
                while typed_count < WARMUP_KEYSTROKES + self.keystrokes && Instant::now() < deadline
                {
                    if typed_count == WARMUP_KEYSTROKES && collector.is_none() {
                        gpui_kit::set_trace_enabled(true);
                        collector = Some(FrameTimingCollector::new());
                    }
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    let jitter = Duration::from_millis(rng % (JITTER_MS + 1));
                    cx.background_executor().timer(BASE_INTERVAL + jitter).await;

                    // GPUI limits an inactive window to 30 frames per second.
                    // Pause rather than take focus back from the user.
                    let active = cx.update(|window, _| window.is_window_active());
                    match active {
                        Ok(true) if !was_active => {
                            was_active = true;
                            // Let frame pacing recover before measuring again.
                            cx.background_executor()
                                .timer(Duration::from_millis(500))
                                .await;
                            continue;
                        }
                        Ok(true) => {}
                        Ok(false) => {
                            if was_active {
                                eprintln!(
                                    "Neon latency bench: paused until the window is frontmost"
                                );
                            }
                            was_active = false;
                            continue;
                        }
                        Err(_) => return,
                    }

                    let key = key_name(typed.next().unwrap_or(' '));
                    let keystroke = Keystroke::parse(key).expect("valid keystroke");
                    let sample = cx.update(|window, cx| {
                        let start = Instant::now();
                        window.dispatch_keystroke(keystroke, cx);
                        KeystrokeSample {
                            start,
                            dispatched: Instant::now(),
                            window_active: window.is_window_active(),
                        }
                    });
                    match sample {
                        Ok(sample) if typed_count >= WARMUP_KEYSTROKES => samples.push(sample),
                        Ok(_) => {}
                        Err(_) => return,
                    }
                    typed_count += 1;
                }
                // Leave time for the last keystroke's frame.
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let events = collector
                    .map(|mut c| c.collect_unseen())
                    .unwrap_or_default();
                gpui_kit::set_trace_enabled(false);

                let report = Report::new(&self.label, window_id, &samples, &events);
                println!("{}", report.to_text());
                if let Some(path) = &self.output
                    && let Err(error) = std::fs::write(path, report.to_json())
                {
                    eprintln!("Couldn’t write {}: {error}", path.display());
                }
                cx.update(|_, cx| cx.quit()).ok();
            })
            .detach();
    }
}

fn key_name(c: char) -> &'static str {
    match c {
        ' ' => "space",
        '.' => ".",
        c => {
            const LETTERS: [&str; 26] = [
                "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p",
                "q", "r", "s", "t", "u", "v", "w", "x", "y", "z",
            ];
            LETTERS[(c as u8 - b'a') as usize]
        }
    }
}

struct KeystrokeSample {
    start: Instant,
    dispatched: Instant,
    /// Inactive windows are frame-rate limited, which skews the result.
    window_active: bool,
}

/// Distribution summary in milliseconds.
#[derive(Default)]
struct Stats {
    mean: f64,
    p50: f64,
    p90: f64,
    p99: f64,
    max: f64,
}

impl Stats {
    fn new(mut values: Vec<f64>) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        values.sort_by(f64::total_cmp);
        let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
        Self {
            mean: values.iter().sum::<f64>() / values.len() as f64,
            p50: at(0.5),
            p90: at(0.9),
            p99: at(0.99),
            max: values[values.len() - 1],
        }
    }

    fn json(&self) -> String {
        format!(
            r#"{{"mean":{:.3},"p50":{:.3},"p90":{:.3},"p99":{:.3},"max":{:.3}}}"#,
            self.mean, self.p50, self.p90, self.p99, self.max
        )
    }
}

struct Report {
    label: String,
    keystrokes: usize,
    matched: usize,
    inactive: usize,
    refresh_hz: Option<f64>,
    /// Dispatch start to frame submitted.
    latency: Stats,
    /// Main-thread time spent on the keystroke: dispatch, then the frame's
    /// draw and submission. Independent of the vsync phase.
    work: Stats,
    /// Key dispatch alone, including any draw it forced first.
    dispatch: Stats,
    /// Every frame drawn while typing.
    draw: Stats,
    draws_per_keystroke: f64,
    /// Share of keystrokes whose dispatch first had to draw a pending frame,
    /// because the window was already invalidated when the key arrived.
    forced_draw_percent: f64,
    /// Mean time per draw in each phase: render and request layout,
    /// layout and prepaint, paint.
    phases: [f64; 3],
    /// Per-keystroke latency in typing order, for plotting.
    latency_samples: Vec<f64>,
    /// Share of keystrokes shown later than the first refresh after dispatch.
    missed_refresh_percent: f64,
}

impl Report {
    fn new(
        label: &str,
        window_id: WindowId,
        samples: &[KeystrokeSample],
        events: &[FrameEvent],
    ) -> Self {
        let draws: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                FrameEvent::Draw(draw) if draw.window_id == window_id => Some(*draw),
                _ => None,
            })
            .collect();
        let presents: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                FrameEvent::Present(present) if present.window_id == window_id => Some(*present),
                _ => None,
            })
            .collect();
        let refresh = draws.iter().find_map(|draw| draw.refresh_interval);

        let ms = |d: Duration| d.as_secs_f64() * 1000.;
        let mut latency = Vec::new();
        let mut work = Vec::new();
        let mut missed = 0;
        for sample in samples.iter().filter(|sample| sample.window_active) {
            let Some(draw) = draws
                .iter()
                .find(|draw| draw.draw_start >= sample.dispatched)
            else {
                continue;
            };
            let Some(present) = presents
                .iter()
                .find(|present| present.present_start >= draw.draw_end)
            else {
                continue;
            };
            let total = present.present_end.duration_since(sample.start);
            latency.push(ms(total));
            work.push(ms(sample.dispatched.duration_since(sample.start)
                + draw.draw_duration()
                + present.present_duration()));
            if let Some(refresh) = refresh
                && total > refresh + Duration::from_micros(500)
            {
                missed += 1;
            }
        }
        let matched = latency.len();
        let forced = samples
            .iter()
            .filter(|sample| {
                draws.iter().any(|draw| {
                    draw.draw_start >= sample.start && draw.draw_start < sample.dispatched
                })
            })
            .count();
        let phase_mean = |phase: fn(&gpui_kit::FrameTiming) -> Duration| {
            draws.iter().map(|draw| ms(phase(draw))).sum::<f64>() / draws.len().max(1) as f64
        };
        let phases = [
            phase_mean(|draw| draw.phases.request_layout),
            phase_mean(|draw| draw.phases.prepaint),
            phase_mean(|draw| draw.phases.paint),
        ];
        let latency_samples = latency.clone();
        Self {
            label: label.to_string(),
            keystrokes: samples.len(),
            matched,
            inactive: samples
                .iter()
                .filter(|sample| !sample.window_active)
                .count(),
            refresh_hz: refresh.map(|interval| 1. / interval.as_secs_f64()),
            latency: Stats::new(latency),
            work: Stats::new(work),
            dispatch: Stats::new(
                samples
                    .iter()
                    .map(|sample| ms(sample.dispatched.duration_since(sample.start)))
                    .collect(),
            ),
            draw: Stats::new(draws.iter().map(|draw| ms(draw.draw_duration())).collect()),
            draws_per_keystroke: draws.len() as f64 / samples.len().max(1) as f64,
            forced_draw_percent: forced as f64 * 100. / samples.len().max(1) as f64,
            phases,
            latency_samples,
            missed_refresh_percent: missed as f64 * 100. / matched.max(1) as f64,
        }
    }

    fn to_text(&self) -> String {
        let mut text = String::new();
        let row = |text: &mut String, name: &str, stats: &Stats| {
            writeln!(
                text,
                "  {name:<22} mean {:>6.2}  p50 {:>6.2}  p90 {:>6.2}  p99 {:>6.2}  max {:>6.2}",
                stats.mean, stats.p50, stats.p90, stats.p99, stats.max
            )
            .ok();
        };
        writeln!(
            text,
            "Neon input latency [{}]: {} of {} keystrokes matched to a frame, display {}",
            self.label,
            self.matched,
            self.keystrokes,
            self.refresh_hz
                .map_or_else(|| "unknown Hz".into(), |hz| format!("{hz:.0} Hz"))
        )
        .ok();
        if self.inactive > 0 {
            writeln!(
                text,
                "  warning: excluded {} keystrokes typed while the window was inactive",
                self.inactive
            )
            .ok();
        }
        writeln!(text, "  (milliseconds)").ok();
        row(&mut text, "key -> frame submitted", &self.latency);
        row(&mut text, "main-thread work", &self.work);
        row(&mut text, "key dispatch", &self.dispatch);
        row(&mut text, "frame draw (all)", &self.draw);
        writeln!(
            text,
            "  draws per keystroke {:.2}, keystrokes later than one refresh {:.1}%",
            self.draws_per_keystroke, self.missed_refresh_percent
        )
        .ok();
        writeln!(
            text,
            "  keystrokes that forced a draw first {:.1}%, draw phases render {:.2} / layout {:.2} / paint {:.2}",
            self.forced_draw_percent, self.phases[0], self.phases[1], self.phases[2]
        )
        .ok();
        text
    }

    fn to_json(&self) -> String {
        format!(
            r#"{{"label":"{}","keystrokes":{},"matched":{},"inactive":{},"refresh_hz":{},"latency_ms":{},"work_ms":{},"dispatch_ms":{},"draw_ms":{},"draws_per_keystroke":{:.3},"missed_refresh_percent":{:.2},"forced_draw_percent":{:.2},"phases_ms":{{"render":{:.3},"layout":{:.3},"paint":{:.3}}},"latency_samples_ms":[{}]}}"#,
            self.label.replace('"', "'"),
            self.keystrokes,
            self.matched,
            self.inactive,
            self.refresh_hz
                .map_or_else(|| "null".into(), |hz| format!("{hz:.1}")),
            self.latency.json(),
            self.work.json(),
            self.dispatch.json(),
            self.draw.json(),
            self.draws_per_keystroke,
            self.missed_refresh_percent,
            self.forced_draw_percent,
            self.phases[0],
            self.phases[1],
            self.phases[2],
            self.latency_samples
                .iter()
                .map(|ms| format!("{ms:.3}"))
                .collect::<Vec<_>>()
                .join(","),
        ) + "\n"
    }
}

/// A large, varied Markdown document, and the 0-based line to type at: the
/// end of a paragraph near the middle.
fn fixture() -> (String, usize) {
    let mut text = String::from("# Latency fixture\n\n");
    let mut cursor_line = 0;
    for section in 0..FIXTURE_SECTIONS {
        write!(
            text,
            "## Section {section}\n\n\
             Paragraph {section} has **bold**, *emphasis*, `code` and a [link](https://example.com).\n\
             It wraps over a few lines so the preview has real text to lay out and the\n\
             editor has real tokens to highlight.\n\n"
        )
        .ok();
        if section == FIXTURE_SECTIONS / 2 {
            cursor_line = text.lines().count() - 2;
        }
        write!(
            text,
            "- First item in section {section}\n\
             - Second item with `inline code`\n\
             - [ ] A task\n\
             - [x] A finished task\n\n\
             ```rust\n\
             fn section_{section}() -> usize {{\n    let value = {section};\n    value * 2\n}}\n\
             ```\n\n\
             | Name | Value | Note |\n\
             | :--- | ----: | :--: |\n\
             | a{section} | {section} | ok |\n\
             | b{section} | {} | ok |\n\n\
             > A quote closing section {section}.\n\n",
            section * 2
        )
        .ok();
    }
    (text, cursor_line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_cursor_is_at_a_paragraph_end() {
        let (text, line) = fixture();
        let target = text.lines().nth(line).unwrap();
        assert!(target.ends_with("editor has real tokens to highlight."));
    }

    #[test]
    fn stats_use_nearest_rank_percentiles() {
        let stats = Stats::new((1..=100).map(f64::from).collect());
        assert_eq!(stats.p50, 51.);
        assert_eq!(stats.p99, 99.);
        assert_eq!(stats.max, 100.);
    }
}
