//! Deterministic real-window workloads for `script/memory-bench`.
//! The external runner samples OS memory at checkpoints and acknowledges each
//! sample. Nothing in this module is compiled into the normal application.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::{App, AsyncWindowContext, Keystroke, WeakEntity, Window};

use super::{LayoutMode, Workspace};

pub struct MemoryBench {
    scenario: String,
    fixtures: PathBuf,
    control: PathBuf,
}

impl MemoryBench {
    pub fn from_env() -> Option<Self> {
        Some(Self {
            scenario: std::env::var("NEON_MEMORY_BENCH").ok()?,
            fixtures: std::env::var_os("NEON_MEMORY_FIXTURES")?.into(),
            control: std::env::var_os("NEON_MEMORY_CONTROL")?.into(),
        })
    }

    pub fn start(self, workspace: WeakEntity<Workspace>, window: &mut Window, cx: &mut App) {
        window
            .spawn(cx, async move |cx| {
                if let Err(error) = self.run(&workspace, cx).await {
                    eprintln!("NEON_MEMORY_ERROR\t{error}");
                    cx.update(|_, cx| cx.quit()).ok();
                }
            })
            .detach();
    }

    async fn run(
        &self,
        workspace: &WeakEntity<Workspace>,
        cx: &mut AsyncWindowContext,
    ) -> anyhow::Result<()> {
        self.settle(workspace, cx).await?;
        match self.scenario.as_str() {
            "startup" => {
                self.checkpoint("empty", workspace, cx).await?;
                workspace.update_in(cx, |workspace, window, cx| {
                    workspace.set_settings_open(true, window, cx)
                })?;
                self.settle(workspace, cx).await?;
                self.checkpoint("settings", workspace, cx).await?;
                workspace.update_in(cx, |workspace, window, cx| {
                    workspace.set_settings_open(false, window, cx)
                })?;
                self.settle(workspace, cx).await?;
                self.checkpoint("settings_closed", workspace, cx).await?;
            }
            "small" | "large" => {
                self.open(&format!("{}.md", self.scenario), workspace, cx)
                    .await?;
                self.checkpoint("loaded", workspace, cx).await?;
                self.type_text(workspace, cx).await?;
                self.checkpoint("typed_split", workspace, cx).await?;
                workspace.update_in(cx, |workspace, window, cx| {
                    workspace.set_layout(LayoutMode::Editor, window, cx)
                })?;
                self.type_text(workspace, cx).await?;
                self.checkpoint("typed_editor", workspace, cx).await?;
                workspace.update_in(cx, |workspace, window, cx| {
                    workspace.set_layout(LayoutMode::Preview, window, cx)
                })?;
                self.settle(workspace, cx).await?;
                self.checkpoint("preview", workspace, cx).await?;
                self.empty(workspace, cx).await?;
                self.checkpoint("released", workspace, cx).await?;
            }
            "images" => {
                for ix in 0..3 {
                    self.open(&format!("image-{ix}.md"), workspace, cx).await?;
                    // Decode/upload is asynchronous and independent of Markdown parsing.
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                    self.checkpoint(&format!("image_{ix}"), workspace, cx)
                        .await?;
                }
                self.empty(workspace, cx).await?;
                self.checkpoint("released", workspace, cx).await?;
            }
            "blocks" => {
                for name in ["list", "table", "code"] {
                    self.open(&format!("{name}.md"), workspace, cx).await?;
                    self.checkpoint(name, workspace, cx).await?;
                }
                self.empty(workspace, cx).await?;
                self.checkpoint("released", workspace, cx).await?;
            }
            "folder" => {
                self.open("folder", workspace, cx).await?;
                self.checkpoint("loaded", workspace, cx).await?;
            }
            scenario => anyhow::bail!("unknown scenario: {scenario}"),
        }
        // Only weak handles are held here, so removing the window can reclaim
        // the workspace and its tasks before the final process-level sample.
        let control = self.control.join("closed.ack");
        cx.update(|window, cx| {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                println!("NEON_MEMORY_CHECKPOINT\tclosed\t0\t0\t0\tclosed\tfalse");
                std::io::stdout().flush().ok();
                let until = Instant::now() + Duration::from_secs(30);
                while !control.exists() && Instant::now() < until {
                    cx.background_executor()
                        .timer(Duration::from_millis(25))
                        .await;
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
            window.remove_window();
        })?;
        Ok(())
    }

    async fn open(
        &self,
        name: &str,
        workspace: &WeakEntity<Workspace>,
        cx: &mut AsyncWindowContext,
    ) -> anyhow::Result<()> {
        let path = self.fixtures.join(name);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_path(path, window, cx)
        })?;
        self.settle(workspace, cx).await
    }

    async fn empty(
        &self,
        workspace: &WeakEntity<Workspace>,
        cx: &mut AsyncWindowContext,
    ) -> anyhow::Result<()> {
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.set_layout(LayoutMode::Split, window, cx);
            workspace.show_document(None, "", window, cx);
        })?;
        self.settle(workspace, cx).await
    }

    async fn type_text(
        &self,
        workspace: &WeakEntity<Workspace>,
        cx: &mut AsyncWindowContext,
    ) -> anyhow::Result<()> {
        let before =
            workspace.read_with(cx, |workspace, cx| workspace.editor().read(cx).text().len())?;
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.focus_content(window, cx)
        })?;
        for _ in 0..40 {
            cx.update(|window, cx| window.dispatch_keystroke(Keystroke::parse("x").unwrap(), cx))?;
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
        }
        self.settle(workspace, cx).await?;
        let after =
            workspace.read_with(cx, |workspace, cx| workspace.editor().read(cx).text().len())?;
        anyhow::ensure!(
            after == before + 40,
            "typing did not insert 40 bytes ({before} -> {after})"
        );
        Ok(())
    }

    async fn settle(
        &self,
        workspace: &WeakEntity<Workspace>,
        cx: &mut AsyncWindowContext,
    ) -> anyhow::Result<()> {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let ready = workspace.read_with(cx, |workspace, cx| {
                !workspace.is_busy()
                    && workspace.folder_task.is_none()
                    && !workspace.preview().read(cx).is_parsing()
            })?;
            if ready {
                break;
            }
            anyhow::ensure!(Instant::now() < until, "document did not settle");
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
        }
        cx.background_executor().timer(Duration::from_secs(1)).await;
        // A real draw makes the workload deterministic when the user briefly
        // covers the benchmark window. Metal submission remains real.
        cx.update(|window, cx| window.draw(cx).clear(cx))?;
        Ok(())
    }

    async fn checkpoint(
        &self,
        name: &str,
        workspace: &WeakEntity<Workspace>,
        cx: &mut AsyncWindowContext,
    ) -> anyhow::Result<()> {
        let (bytes, blocks, layout) = workspace.read_with(cx, |workspace, cx| {
            (
                workspace.editor().read(cx).text().len(),
                workspace.preview().read(cx).document().blocks().len(),
                format!("{:?}", workspace.layout),
            )
        })?;
        let (scale, active) =
            cx.update(|window, _| (window.scale_factor(), window.is_window_active()))?;
        println!("NEON_MEMORY_CHECKPOINT\t{name}\t{bytes}\t{blocks}\t{scale}\t{layout}\t{active}");
        std::io::stdout().flush()?;
        let acknowledgement = self.control.join(format!("{name}.ack"));
        let until = Instant::now() + Duration::from_secs(30);
        while !acknowledgement.exists() {
            anyhow::ensure!(Instant::now() < until, "runner did not acknowledge {name}");
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
        }
        Ok(())
    }
}
