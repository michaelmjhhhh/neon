use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::{Context, Entity, IntoElement, Render, Styled as _, Window};

use crate::typography::Typography;

/// The source editor as its own view, so the workspace can cache it: a
/// keystroke re-renders this view alone, and a preview update or status
/// change reuses the editor's last frame instead of laying it out again.
pub(crate) struct EditorPane {
    editor: Entity<EditorState>,
    typography: Entity<Typography>,
    readonly: bool,
}

impl EditorPane {
    pub(crate) fn new(editor: Entity<EditorState>, typography: Entity<Typography>) -> Self {
        Self {
            editor,
            typography,
            readonly: false,
        }
    }

    pub(crate) fn set_readonly(&mut self, readonly: bool, cx: &mut Context<Self>) {
        if self.readonly != readonly {
            self.readonly = readonly;
            cx.notify();
        }
    }
}

impl Render for EditorPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let typography = self.typography.read(cx);
        Editor::new(&self.editor)
            .readonly(self.readonly)
            .font_family(typography.editor_font(cx))
            .text_size(gpui_kit::rems(typography.editor_size(cx) / 16.))
            // The workspace sheet draws the boundary; the editor stays flat.
            .bordered(false)
            .size_full()
    }
}
