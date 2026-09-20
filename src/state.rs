use crate::model::{FixtureError, FixtureSet, Folder, FolderId, FolderKind, Message, MessageId};

#[derive(Clone, Debug)]
pub struct AppState {
    fixtures: FixtureSet,
    selected_folder_id: FolderId,
    selected_message_id: Option<MessageId>,
    search_query: String,
    message_filter: MessageFilter,
    surface: Surface,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    SelectFolder(FolderId),
    SelectMessage(MessageId),
    SetSearch(String),
    SetFilter(MessageFilter),
    SelectNext,
    SelectPrevious,
    ArchiveSelected,
    SetSurface(Surface),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Transition {
    pub folders_changed: bool,
    pub list_changed: bool,
    pub reader_changed: bool,
    pub feedback: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Surface {
    #[default]
    Online,
    Loading,
    Offline,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MessageFilter {
    #[default]
    All,
    Unread,
    Attachments,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewStatus {
    Ready,
    Loading,
    Offline,
    EmptyFolder,
    NoSearchResults,
}

#[derive(Clone, Debug)]
pub struct ViewSnapshot {
    pub folders: Vec<Folder>,
    pub visible_messages: Vec<Message>,
    pub selected_message: Option<Message>,
    pub selected_folder_id: FolderId,
    pub search_query: String,
    pub message_filter: MessageFilter,
    pub surface: Surface,
    pub status: ViewStatus,
    pub folder_counts: Vec<(FolderId, usize)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EscapeContext {
    pub search_active: bool,
    pub reader_visible: bool,
    pub folders_visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EscapeOutcome {
    ClearSearch,
    ShowMessageList,
    HideFolders,
    None,
}

pub fn escape_outcome(context: EscapeContext) -> EscapeOutcome {
    if context.search_active {
        EscapeOutcome::ClearSearch
    } else if context.reader_visible {
        EscapeOutcome::ShowMessageList
    } else if context.folders_visible {
        EscapeOutcome::HideFolders
    } else {
        EscapeOutcome::None
    }
}

impl AppState {
    pub fn new(fixtures: FixtureSet) -> Result<Self, FixtureError> {
        fixtures.validate()?;
        let selected_folder_id = fixtures
            .folders
            .first()
            .map_or(FolderId::INBOX, |folder| folder.id);
        let mut state = Self {
            fixtures,
            selected_folder_id,
            selected_message_id: None,
            search_query: String::new(),
            message_filter: MessageFilter::All,
            surface: Surface::Online,
        };
        state.normalize_selection(None);
        Ok(state)
    }

    pub fn dispatch(&mut self, action: Action) -> Transition {
        match action {
            Action::SelectFolder(id) => self.select_folder(id),
            Action::SelectMessage(id) => self.select_message(id),
            Action::SetSearch(query) => self.set_search(query),
            Action::SetFilter(filter) => self.set_filter(filter),
            Action::SelectNext => self.move_selection(1),
            Action::SelectPrevious => self.move_selection(-1),
            Action::ArchiveSelected => self.archive_selected(),
            Action::SetSurface(surface) => {
                self.surface = surface;
                Transition {
                    list_changed: true,
                    reader_changed: true,
                    ..Transition::default()
                }
            }
        }
    }

    pub fn snapshot(&self) -> ViewSnapshot {
        ViewSnapshot {
            folders: self.fixtures.folders.clone(),
            visible_messages: self.visible_messages().into_iter().cloned().collect(),
            selected_message: self.selected_message().cloned(),
            selected_folder_id: self.selected_folder_id,
            search_query: self.search_query.clone(),
            message_filter: self.message_filter,
            surface: self.surface,
            status: self.status(),
            folder_counts: self
                .fixtures
                .folders
                .iter()
                .map(|folder| (folder.id, self.folder_count(folder.id)))
                .collect(),
        }
    }

    pub fn selected_folder_id(&self) -> FolderId {
        self.selected_folder_id
    }
    pub fn selected_message_id(&self) -> Option<MessageId> {
        self.selected_message_id
    }

    pub fn selected_message(&self) -> Option<&Message> {
        let id = self.selected_message_id?;
        self.fixtures
            .messages
            .iter()
            .find(|message| message.id == id)
    }

    pub fn visible_message_ids(&self) -> Vec<MessageId> {
        self.visible_messages()
            .into_iter()
            .map(|message| message.id)
            .collect()
    }

    pub fn visible_messages(&self) -> Vec<&Message> {
        let query = self.search_query.trim().to_lowercase();
        self.fixtures
            .messages
            .iter()
            .filter(|message| {
                self.message_in_selected_folder(message)
                    && self.message_matches_filter(message)
                    && (query.is_empty()
                        || message.sender.to_lowercase().contains(&query)
                        || message.subject.to_lowercase().contains(&query)
                        || message
                            .preview
                            .is_some_and(|preview| preview.to_lowercase().contains(&query)))
            })
            .collect()
    }

    pub fn folder_count(&self, id: FolderId) -> usize {
        let Some(folder) = self.fixtures.folders.iter().find(|folder| folder.id == id) else {
            return 0;
        };
        match folder.kind {
            FolderKind::Mailbox if id == FolderId::INBOX => self
                .fixtures
                .messages
                .iter()
                .filter(|message| message.folder_id == id && message.unread)
                .count(),
            FolderKind::Mailbox => self
                .fixtures
                .messages
                .iter()
                .filter(|message| message.folder_id == id)
                .count(),
            FolderKind::Starred => self
                .fixtures
                .messages
                .iter()
                .filter(|message| message.starred)
                .count(),
        }
    }

    pub fn status(&self) -> ViewStatus {
        match self.surface {
            Surface::Loading => ViewStatus::Loading,
            Surface::Offline => ViewStatus::Offline,
            Surface::Online
                if self.visible_messages().is_empty()
                    && (!self.search_query.is_empty()
                        || self.message_filter != MessageFilter::All) =>
            {
                ViewStatus::NoSearchResults
            }
            Surface::Online if self.visible_messages().is_empty() => ViewStatus::EmptyFolder,
            Surface::Online => ViewStatus::Ready,
        }
    }

    fn message_in_selected_folder(&self, message: &Message) -> bool {
        self.fixtures
            .folders
            .iter()
            .find(|folder| folder.id == self.selected_folder_id)
            .is_some_and(|folder| match folder.kind {
                FolderKind::Mailbox => message.folder_id == folder.id,
                FolderKind::Starred => message.starred,
            })
    }

    fn message_matches_filter(&self, message: &Message) -> bool {
        match self.message_filter {
            MessageFilter::All => true,
            MessageFilter::Unread => message.unread,
            MessageFilter::Attachments => !message.attachments.is_empty(),
        }
    }

    fn select_folder(&mut self, id: FolderId) -> Transition {
        if !self.fixtures.folders.iter().any(|folder| folder.id == id) {
            return Transition {
                feedback: Some("Folder is unavailable"),
                ..Transition::default()
            };
        }
        self.selected_folder_id = id;
        self.search_query.clear();
        self.message_filter = MessageFilter::All;
        self.normalize_selection(None);
        Transition {
            folders_changed: true,
            list_changed: true,
            reader_changed: true,
            ..Transition::default()
        }
    }

    fn select_message(&mut self, id: MessageId) -> Transition {
        if !self.visible_message_ids().contains(&id) {
            return Transition {
                feedback: Some("Message is unavailable"),
                ..Transition::default()
            };
        }
        self.selected_message_id = Some(id);
        Transition {
            reader_changed: true,
            ..Transition::default()
        }
    }

    fn set_search(&mut self, query: String) -> Transition {
        self.search_query = query.trim().to_owned();
        self.normalize_selection(None);
        Transition {
            list_changed: true,
            reader_changed: true,
            ..Transition::default()
        }
    }

    fn set_filter(&mut self, filter: MessageFilter) -> Transition {
        self.message_filter = filter;
        self.normalize_selection(None);
        Transition {
            list_changed: true,
            reader_changed: true,
            ..Transition::default()
        }
    }

    fn move_selection(&mut self, direction: isize) -> Transition {
        let visible = self.visible_message_ids();
        if visible.is_empty() {
            return Transition::default();
        }
        let current = self
            .selected_message_id
            .and_then(|id| visible.iter().position(|candidate| *candidate == id));
        let index = match (current, direction) {
            (Some(index), step) => index.saturating_add_signed(step).min(visible.len() - 1),
            (None, step) if step < 0 => visible.len() - 1,
            (None, _) => 0,
        };
        self.selected_message_id = Some(visible[index]);
        Transition {
            reader_changed: true,
            ..Transition::default()
        }
    }

    fn archive_selected(&mut self) -> Transition {
        let Some(id) = self.selected_message_id else {
            return Transition {
                feedback: Some("No message selected"),
                ..Transition::default()
            };
        };
        let preferred = self
            .visible_message_ids()
            .iter()
            .position(|candidate| *candidate == id);
        let Some(message) = self
            .fixtures
            .messages
            .iter_mut()
            .find(|message| message.id == id)
        else {
            self.normalize_selection(None);
            return Transition {
                feedback: Some("Message is unavailable"),
                list_changed: true,
                reader_changed: true,
                ..Transition::default()
            };
        };
        message.folder_id = FolderId::ARCHIVE;
        self.normalize_selection(preferred);
        Transition {
            folders_changed: true,
            list_changed: true,
            reader_changed: true,
            feedback: Some("Message archived"),
        }
    }

    fn normalize_selection(&mut self, preferred: Option<usize>) {
        let visible = self.visible_message_ids();
        if self
            .selected_message_id
            .is_some_and(|id| visible.contains(&id))
        {
            return;
        }
        self.selected_message_id = if visible.is_empty() {
            None
        } else {
            Some(visible[preferred.unwrap_or(0).min(visible.len() - 1)])
        };
    }
}

#[cfg(test)]
mod tests;
