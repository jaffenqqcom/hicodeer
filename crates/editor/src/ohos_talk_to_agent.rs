// OHOS-only: collects the editor selection for the Talk to Agent dialog.
//
// This lives in the editor crate because the selection must be read from the
// exact Editor instance the user acted on. `SplittableEditor::act_as_type`
// always resolves to the right-hand editor, so reading the selection from the
// workspace would return the wrong side of a git diff.

use gpui::{Context, Window};
use language::ToPoint as _;
use zed_actions::agent::{SendSelectionToAgent, TalkToAgent};

use crate::Editor;

impl Editor {
    /// Reads the newest selection, maps it back to file coordinates and asks
    /// the Agent panel to open the Talk to Agent dialog.
    pub(crate) fn talk_to_agent(
        &mut self,
        _: &TalkToAgent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let display_snapshot = self.display_snapshot(cx);
        let selections = self.selections.all_adjusted(&display_snapshot);
        let Some(selection) = selections.last().copied() else {
            log::warn!("talk_to_agent: the editor has no selection");
            return;
        };
        // An empty selection is allowed: the dialog then reports the caret
        // line and shows an empty preview.
        let selection_is_empty = selection.is_empty();

        let multi_buffer = self.buffer().clone();
        let (start_buffer, start_anchor, end_buffer, end_anchor) = {
            let snapshot = multi_buffer.read(cx).snapshot(cx);
            // Bias the start forward so a selection that begins on the first
            // line of an excerpt still resolves to that excerpt, not the one
            // before it.
            let start_position = if selection_is_empty {
                snapshot.anchor_before(selection.start)
            } else {
                snapshot.anchor_after(selection.start)
            };
            let end_position = snapshot.anchor_before(selection.end);
            let Some(start) = multi_buffer
                .read(cx)
                .text_anchor_for_position(start_position, cx)
            else {
                log::warn!("talk_to_agent: selection start is not inside an excerpt");
                return;
            };
            let Some(end) = multi_buffer
                .read(cx)
                .text_anchor_for_position(end_position, cx)
            else {
                log::warn!("talk_to_agent: selection end is not inside an excerpt");
                return;
            };
            (start.0, start.1, end.0, end.1)
        };

        if start_buffer != end_buffer {
            log::warn!("talk_to_agent: the selection spans more than one file");
            return;
        }

        let selected_text = if selection_is_empty {
            String::new()
        } else {
            multi_buffer
                .read(cx)
                .snapshot(cx)
                .text_for_range(selection.start..selection.end)
                .collect::<String>()
        };

        let language_name = start_buffer
            .read(cx)
            .language()
            .map(|language| language.name().to_string())
            .unwrap_or_default();

        let buffer_snapshot = start_buffer.read(cx).snapshot();
        let start_point = start_anchor.to_point(&buffer_snapshot);
        let end_point = end_anchor.to_point(&buffer_snapshot);

        // The end anchor is exclusive. A selection that stops at the start of
        // the next line covers the previous line in full, so report that line
        // instead of the one the caret sits on.
        let end_row = if end_point.column == 0 && end_point.row > start_point.row {
            end_point.row - 1
        } else {
            end_point.row
        };

        let is_before_change = self.diff_hunk_action_target.is_some();
        let mut file_path = self.target_file_abs_path(cx);

        // The left side of a diff is a read-only Buffer::local(base_text) with
        // no file of its own. Its path comes from the right-hand editor that
        // the left editor keeps as its diff hunk action target.
        if is_before_change
            && let Some(right_editor) = self
                .diff_hunk_action_target
                .as_ref()
                .and_then(|target| target.upgrade())
        {
            let right_path = right_editor.update(cx, |editor, cx| editor.target_file_abs_path(cx));
            if right_path.is_some() {
                file_path = right_path;
            }
        }

        // An unsaved buffer has no path: the dialog still opens, it just sends
        // the selected text without a file reference or a line range.
        let file_path = file_path
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();

        window.dispatch_action(
            Box::new(SendSelectionToAgent {
                file_path,
                start_line: start_point.row as u32 + 1,
                end_line: end_row as u32 + 1,
                selected_text,
                language_name,
                is_before_change,
            }),
            cx,
        );
    }
}
