use std::collections::VecDeque;

pub const MAX_HISTORY_COMMANDS: usize = 1_000;
pub const MAX_HISTORY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditCommand {
    Cell {
        row: usize,
        column: usize,
        before: String,
        after: String,
    },
    Source {
        start: usize,
        before: String,
        after: String,
    },
}

impl EditCommand {
    fn bytes(&self) -> usize {
        match self {
            Self::Cell { before, after, .. } | Self::Source { before, after, .. } => {
                before.len() + after.len()
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct UndoStack {
    undo: VecDeque<EditCommand>,
    redo: Vec<EditCommand>,
    bytes: usize,
}

impl UndoStack {
    pub fn push(&mut self, command: EditCommand) {
        self.redo.clear();
        self.bytes += command.bytes();
        self.undo.push_back(command);
        while self.undo.len() > MAX_HISTORY_COMMANDS || self.bytes > MAX_HISTORY_BYTES {
            if let Some(old) = self.undo.pop_front() {
                self.bytes = self.bytes.saturating_sub(old.bytes());
            }
        }
    }

    pub fn undo(&mut self) -> Option<EditCommand> {
        let command = self.undo.pop_back()?;
        self.bytes = self.bytes.saturating_sub(command.bytes());
        self.redo.push(command.clone());
        Some(command)
    }

    pub fn redo(&mut self) -> Option<EditCommand> {
        let command = self.redo.pop()?;
        self.bytes += command.bytes();
        self.undo.push_back(command.clone());
        Some(command)
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.bytes = 0;
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
