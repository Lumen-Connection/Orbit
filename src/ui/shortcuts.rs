//! Central keyboard shortcut table. Widgets must not bind these ad hoc.

use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutId {
    NewSession,
    Search,
    Cancel,
    NextSession,
    PrevSession,
    Settings,
    Send,
    FontBigger,
    FontSmaller,
    FontReset,
}

#[derive(Debug, Clone, Copy)]
pub struct Shortcut {
    pub id: ShortcutId,
    pub label: &'static str,
    pub keys: &'static str,
    /// Still fires while a text field has focus.
    pub while_typing: bool,
}

pub const SHORTCUTS: &[Shortcut] = &[
    Shortcut {
        id: ShortcutId::NewSession,
        label: "New conversation / session",
        keys: "Ctrl+N",
        while_typing: true,
    },
    Shortcut {
        id: ShortcutId::Search,
        label: "Search history",
        keys: "Ctrl+K",
        while_typing: true,
    },
    Shortcut {
        id: ShortcutId::Cancel,
        label: "Cancel generation",
        keys: "Esc",
        while_typing: true,
    },
    Shortcut {
        id: ShortcutId::NextSession,
        label: "Next session",
        keys: "Ctrl+Tab",
        while_typing: true,
    },
    Shortcut {
        id: ShortcutId::PrevSession,
        label: "Previous session",
        keys: "Ctrl+Shift+Tab",
        while_typing: true,
    },
    Shortcut {
        id: ShortcutId::Settings,
        label: "Settings",
        keys: "Ctrl+,",
        while_typing: true,
    },
    Shortcut {
        id: ShortcutId::Send,
        label: "Send message",
        keys: "Ctrl+Enter",
        while_typing: true,
    },
    Shortcut {
        id: ShortcutId::FontBigger,
        label: "Increase font scale",
        keys: "Ctrl+=",
        while_typing: true,
    },
    Shortcut {
        id: ShortcutId::FontSmaller,
        label: "Decrease font scale",
        keys: "Ctrl+-",
        while_typing: true,
    },
    Shortcut {
        id: ShortcutId::FontReset,
        label: "Reset font scale",
        keys: "Ctrl+0",
        while_typing: true,
    },
];

pub fn consume(ctx: &egui::Context, text_focused: bool) -> Option<ShortcutId> {
    ctx.input_mut(|input| {
        for shortcut in SHORTCUTS {
            if text_focused && !shortcut.while_typing {
                continue;
            }
            if matches(input, shortcut.id) {
                return Some(shortcut.id);
            }
        }
        None
    })
}

fn matches(input: &mut egui::InputState, id: ShortcutId) -> bool {
    let cmd = egui::Modifiers::COMMAND;
    let cmd_shift = egui::Modifiers {
        shift: true,
        ..egui::Modifiers::COMMAND
    };
    match id {
        ShortcutId::NewSession => input.consume_key(cmd, egui::Key::N),
        ShortcutId::Search => input.consume_key(cmd, egui::Key::K),
        ShortcutId::Cancel => input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        ShortcutId::NextSession => input.consume_key(cmd, egui::Key::Tab),
        ShortcutId::PrevSession => input.consume_key(cmd_shift, egui::Key::Tab),
        ShortcutId::Settings => input.consume_key(cmd, egui::Key::Comma),
        ShortcutId::Send => input.consume_key(cmd, egui::Key::Enter),
        ShortcutId::FontBigger => {
            input.consume_key(cmd, egui::Key::Equals) || input.consume_key(cmd, egui::Key::Plus)
        }
        ShortcutId::FontSmaller => input.consume_key(cmd, egui::Key::Minus),
        ShortcutId::FontReset => input.consume_key(cmd, egui::Key::Num0),
    }
}

#[cfg(test)]
mod tests {
    use super::{SHORTCUTS, ShortcutId};

    #[test]
    fn table_covers_required_bindings() {
        let ids: Vec<ShortcutId> = SHORTCUTS.iter().map(|s| s.id).collect();
        for required in [
            ShortcutId::NewSession,
            ShortcutId::Search,
            ShortcutId::Cancel,
            ShortcutId::NextSession,
            ShortcutId::PrevSession,
            ShortcutId::Settings,
            ShortcutId::Send,
            ShortcutId::FontBigger,
            ShortcutId::FontSmaller,
            ShortcutId::FontReset,
        ] {
            assert!(ids.contains(&required), "missing {required:?}");
        }
        assert!(
            SHORTCUTS
                .iter()
                .find(|s| s.id == ShortcutId::Search)
                .is_some_and(|s| s.keys == "Ctrl+K")
        );
    }
}
