//! PDF export: WebKit prints the exported HTML page, in a child process.
//!
//! Printing the same page as the HTML export makes the two formats match.
//! WebKit loads pages asynchronously and needs the main run loop to turn
//! while it does; running that loop inside the app would hand GPUI events
//! back to code that is mid-update. So the app starts its own executable
//! again with [`WORKER_ARG`], and that process, with nothing else on its
//! main thread, does the layout. The parent waits with a time limit and moves the PDF
//! into place only after the worker reports success and the file checks out.

use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

use crate::{ExportSource, ExportStyle, MARGIN_PT, temp_path_beside};

/// Starts the executable as the PDF worker rather than the app.
pub const WORKER_ARG: &str = "--export-pdf-worker";

/// Marks the worker's error report among WebKit's own log lines.
const ERROR_PREFIX: &str = "neon-export-error: ";

/// How long the worker may take before the export is abandoned. Generous:
/// a long document with many images can take several seconds to lay out.
const TIMEOUT: Duration = Duration::from_secs(120);

/// Export the document to `path`, laid out by a worker started from
/// `executable`. Blocks until the worker finishes, so call it off the main
/// thread.
pub fn export(
    source: &ExportSource,
    style: &ExportStyle,
    path: &Path,
    executable: &Path,
) -> Result<()> {
    let html = crate::render_html(source, style);
    let temp = temp_path_beside(path)?;
    // Creating the file here fails fast on a missing or read-only folder,
    // which AppKit would otherwise report with an alert no one can see.
    std::fs::File::create(&temp)
        .with_context(|| format!("couldn’t write to {}", folder_of(path).display()))?;
    let result = run_worker_process(executable, html, style.paper, &temp)
        .and_then(|()| check_pdf(&temp))
        .and_then(|()| {
            std::fs::rename(&temp, path)
                .with_context(|| format!("couldn’t write {}", path.display()))
        });
    if result.is_err() {
        std::fs::remove_file(&temp).ok();
    }
    result
}

fn folder_of(path: &Path) -> &Path {
    path.parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn run_worker_process(
    executable: &Path,
    html: String,
    paper: (f32, f32),
    output: &Path,
) -> Result<()> {
    let mut child = Command::new(executable)
        .arg(WORKER_ARG)
        .arg(output)
        .arg(paper.0.to_string())
        .arg(paper.1.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("couldn’t start the PDF exporter")?;

    // Drain stderr on its own thread so a chatty worker can't fill the pipe
    // and stall.
    let mut stderr = child.stderr.take().context("no worker stderr")?;
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        stderr.read_to_string(&mut text).ok();
        text
    });
    // Send the page from its own thread too, so a worker that stops reading
    // can't hold the export past the time limit.
    let mut stdin = child.stdin.take().context("no worker stdin")?;
    let written = std::thread::spawn(move || stdin.write_all(html.as_bytes()));

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > TIMEOUT {
            child.kill().ok();
            child.wait().ok();
            bail!("laying out the PDF took too long");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let errors = errors.join().unwrap_or_default();
    if !status.success() {
        // WebKit logs to stderr as well; only the worker's own report counts.
        let message = errors
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix(ERROR_PREFIX));
        match message {
            Some(message) => bail!("{}", message.trim()),
            None => bail!("the PDF exporter stopped unexpectedly ({status})"),
        }
    }
    match written.join() {
        Ok(Ok(())) => Ok(()),
        _ => bail!("couldn’t send the document to the PDF exporter"),
    }
}

/// Trust the worker's file only if it is a complete PDF.
fn check_pdf(path: &Path) -> Result<()> {
    let bytes = std::fs::read(path).context("the PDF exporter wrote no file")?;
    let tail = &bytes[bytes.len().saturating_sub(1024)..];
    let complete = bytes.starts_with(b"%PDF-") && tail.windows(5).any(|window| window == b"%%EOF");
    if !complete {
        bail!("the PDF exporter wrote an incomplete file");
    }
    Ok(())
}

/// Run as the PDF worker when started with [`WORKER_ARG`]: returns the exit
/// code to quit with, or `None` for a normal launch. Call first thing in
/// `main`, before the app starts.
pub fn run_worker_if_requested() -> Option<i32> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first()? != WORKER_ARG {
        return None;
    }
    Some(match worker(&args[1..]) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{ERROR_PREFIX}{error:#}");
            1
        }
    })
}

fn worker(args: &[OsString]) -> Result<()> {
    let [output, width, height] = args else {
        bail!("the PDF exporter was started with the wrong arguments");
    };
    let parse = |value: &OsString| -> Result<f64> {
        let value: f64 = value
            .to_str()
            .and_then(|value| value.parse().ok())
            .context("the PDF exporter was given an invalid paper size")?;
        if !(144. ..=14400.).contains(&value) {
            bail!("the PDF exporter was given an invalid paper size");
        }
        Ok(value)
    };
    let paper = (parse(width)?, parse(height)?);
    let mut html = Vec::new();
    std::io::stdin()
        .read_to_end(&mut html)
        .context("couldn’t read the document")?;
    macos::print_html_to_pdf(&html, paper, Path::new(output))
}

/// The paper size chosen in the system's page setup, in points. Call on the
/// main thread.
pub fn default_paper() -> Option<(f32, f32)> {
    macos::default_paper()
}

mod macos {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::path::Path;
    use std::time::Instant;

    use anyhow::{Context as _, Result, anyhow, bail};
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObject, NSObjectProtocol};
    use objc2::{DefinedClass as _, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSApplication, NSApplicationActivationPolicy,
        NSBackingStoreType, NSPrintInfo, NSPrintJobSavingURL, NSPrintOperation, NSPrintSaveJob,
        NSPrintingPaginationMode, NSWindow, NSWindowStyleMask,
    };
    use objc2_foundation::{
        NSDate, NSDefaultRunLoopMode, NSError, NSPoint, NSRect, NSRunLoop, NSSize, NSString, NSURL,
    };
    use objc2_web_kit::{WKNavigation, WKNavigationDelegate, WKWebView, WKWebViewConfiguration};

    use super::{MARGIN_PT, TIMEOUT};

    pub(super) fn default_paper() -> Option<(f32, f32)> {
        MainThreadMarker::new()?;
        let size = NSPrintInfo::sharedPrintInfo().paperSize();
        (size.width >= 144. && size.height >= 144.)
            .then_some((size.width as f32, size.height as f32))
    }

    /// What WebKit and AppKit have reported back so far.
    #[derive(Default)]
    struct Progress {
        /// How the page load ended.
        loaded: Option<Result<(), String>>,
        /// Whether printing succeeded, once it has finished.
        printed: Option<bool>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "NeonPdfNavigationDelegate"]
        #[ivars = RefCell<Progress>]
        struct NavigationDelegate;

        impl NavigationDelegate {
            #[unsafe(method(printOperationDidRun:success:contextInfo:))]
            fn print_operation_did_run(
                &self,
                _: &NSPrintOperation,
                success: Bool,
                _: *mut c_void,
            ) {
                self.ivars().borrow_mut().printed = Some(success.as_bool());
            }
        }

        unsafe impl NSObjectProtocol for NavigationDelegate {}

        unsafe impl WKNavigationDelegate for NavigationDelegate {
            #[unsafe(method(webView:didFinishNavigation:))]
            fn did_finish(&self, _: &WKWebView, _: Option<&WKNavigation>) {
                self.finish(Ok(()));
            }

            #[unsafe(method(webView:didFailNavigation:withError:))]
            fn did_fail(&self, _: &WKWebView, _: Option<&WKNavigation>, error: &NSError) {
                self.finish(Err(error.localizedDescription().to_string()));
            }

            #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
            fn did_fail_provisional(
                &self,
                _: &WKWebView,
                _: Option<&WKNavigation>,
                error: &NSError,
            ) {
                self.finish(Err(error.localizedDescription().to_string()));
            }

            #[unsafe(method(webViewWebContentProcessDidTerminate:))]
            fn content_process_terminated(&self, _: &WKWebView) {
                self.finish(Err("WebKit stopped while laying out the document".into()));
            }
        }
    );

    impl NavigationDelegate {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(RefCell::new(Progress::default()));
            unsafe { msg_send![super(this), init] }
        }

        fn finish(&self, result: Result<(), String>) {
            self.ivars().borrow_mut().loaded.get_or_insert(result);
        }

        fn loaded(&self) -> Option<Result<(), String>> {
            self.ivars().borrow().loaded.clone()
        }

        fn printed(&self) -> Option<bool> {
            self.ivars().borrow().printed
        }
    }

    pub(super) fn print_html_to_pdf(html: &[u8], paper: (f64, f64), output: &Path) -> Result<()> {
        let mtm =
            MainThreadMarker::new().context("the PDF exporter must run on the main thread")?;
        let html = std::str::from_utf8(html).context("the document isn’t valid UTF-8")?;
        // A background process: no Dock icon or menu bar, and the light
        // appearance whatever the system uses.
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
        app.setAppearance(
            NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }).as_deref(),
        );

        let margin = MARGIN_PT as f64;
        let page = NSSize::new(paper.0 - 2. * margin, paper.1 - 2. * margin);
        let frame = NSRect::new(NSPoint::new(0., 0.), page);
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        let preferences = unsafe { configuration.preferences() };
        // Code blocks and table headers keep their shading. The setting is
        // new in macOS 13.3; the stylesheet asks for the same on older ones.
        if preferences.respondsToSelector(sel!(setShouldPrintBackgrounds:)) {
            unsafe { preferences.setShouldPrintBackgrounds(true) };
        }
        let web_view = unsafe {
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &configuration)
        };
        let delegate = NavigationDelegate::new(mtm);
        unsafe {
            web_view
                .setNavigationDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate)))
        };
        // The print operation runs against a window. It is never shown.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setContentView(Some(&web_view));

        unsafe { web_view.loadHTMLString_baseURL(&NSString::from_str(html), None) };
        let deadline = Instant::now() + TIMEOUT;
        run_until(deadline, || delegate.loaded())
            .context("loading the document took too long")?
            .map_err(|error| anyhow!("couldn’t lay out the document: {error}"))?;

        let info = NSPrintInfo::new();
        info.setPaperSize(NSSize::new(paper.0, paper.1));
        info.setTopMargin(margin);
        info.setBottomMargin(margin);
        info.setLeftMargin(margin);
        info.setRightMargin(margin);
        info.setHorizontalPagination(NSPrintingPaginationMode::Fit);
        info.setVerticalPagination(NSPrintingPaginationMode::Automatic);
        info.setHorizontallyCentered(false);
        info.setVerticallyCentered(false);
        info.setJobDisposition(unsafe { NSPrintSaveJob });
        let url = NSURL::fileURLWithPath(&NSString::from_str(&output.to_string_lossy()));
        unsafe { info.dictionary().insert(NSPrintJobSavingURL, &*url) };

        let operation = unsafe { web_view.printOperationWithPrintInfo(&info) };
        operation.setShowsPrintPanel(false);
        operation.setShowsProgressPanel(false);
        // WebKit lays out print pages asynchronously; the synchronous
        // `runOperation` never finishes. The modal form reports back to the
        // delegate instead.
        unsafe {
            operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
                &window,
                Some(&delegate),
                Some(sel!(printOperationDidRun:success:contextInfo:)),
                std::ptr::null_mut(),
            )
        };
        let printed = run_until(deadline, || delegate.printed())
            .context("laying out the PDF took too long")?;
        if !printed {
            bail!("couldn’t write the PDF");
        }
        Ok(())
    }

    /// Turn the run loop, so WebKit can work and call back, until `done`
    /// has an answer or the deadline passes.
    fn run_until<T>(deadline: Instant, mut done: impl FnMut() -> Option<T>) -> Option<T> {
        let run_loop = NSRunLoop::currentRunLoop();
        loop {
            if let Some(value) = done() {
                return Some(value);
            }
            if Instant::now() > deadline {
                return None;
            }
            let until = NSDate::dateWithTimeIntervalSinceNow(0.05);
            unsafe { run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &until) };
        }
    }
}

/// Where a test or a caller can find the exporter executable.
pub fn current_executable() -> Result<PathBuf> {
    std::env::current_exe().context("couldn’t locate Neon to start the PDF exporter")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export_with(executable: &str) -> (Result<()>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let source = ExportSource {
            markdown: "# Hi",
            title: "Hi",
            base_dir: None,
        };
        let path = dir.path().join("out.pdf");
        let result = export(
            &source,
            &ExportStyle::default(),
            &path,
            Path::new(executable),
        );
        (result, dir)
    }

    fn files(dir: &tempfile::TempDir) -> usize {
        std::fs::read_dir(dir.path()).unwrap().count()
    }

    #[test]
    fn reports_a_worker_that_fails() {
        let (result, dir) = export_with("/usr/bin/false");
        let error = result.unwrap_err().to_string();
        assert!(error.contains("stopped unexpectedly"), "{error}");
        assert_eq!(files(&dir), 0);
    }

    #[test]
    fn rejects_a_worker_that_writes_no_pdf() {
        // The worker must read the whole document before exiting: one that
        // exits first, like `/usr/bin/true`, races the parent's write and
        // fails the export with a broken pipe instead.
        let scripts = tempfile::tempdir().unwrap();
        let worker = scripts.path().join("worker");
        std::fs::write(&worker, "#!/bin/sh\ncat > /dev/null\n").unwrap();
        std::fs::set_permissions(&worker, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let (result, dir) = export_with(worker.to_str().unwrap());
        let error = result.unwrap_err().to_string();
        assert!(error.contains("incomplete file"), "{error}");
        assert_eq!(files(&dir), 0);
    }

    #[test]
    fn reports_a_worker_that_cannot_start() {
        let (result, dir) = export_with("/nonexistent/neon");
        assert!(result.unwrap_err().to_string().contains("couldn’t start"));
        assert_eq!(files(&dir), 0);
    }
}
