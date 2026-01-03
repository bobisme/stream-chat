use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct ChatMessage {
    #[allow(dead_code)]
    pub id: String,
    pub author_name: String,
    pub message: String,
    pub author_type: AuthorType,
    pub super_chat: Option<SuperChatInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorType {
    Owner,
    Moderator,
    Member,
    Regular,
}

impl AuthorType {
    pub fn color(self) -> Color {
        match self {
            AuthorType::Owner => Color::Yellow,
            AuthorType::Moderator => Color::Blue,
            AuthorType::Member => Color::Green,
            AuthorType::Regular => Color::White,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SuperChatInfo {
    #[allow(dead_code)]
    pub amount_display: String,
    pub tier: u32,
}
