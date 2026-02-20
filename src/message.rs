use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct ChatMessage {
    #[allow(dead_code)]
    pub id: String,
    pub author_name: String,
    pub message: String,
    pub author_type: AuthorType,
    pub badges: Vec<Badge>,
    #[allow(dead_code)]
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
    pub const fn color(self) -> Color {
        match self {
            Self::Owner => Color::Yellow,
            Self::Moderator => Color::Blue,
            Self::Member => Color::Green,
            Self::Regular => Color::DarkGray,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    Owner,
    Moderator,
    Member,
    Rank,
    Other,
}

#[derive(Debug, Clone)]
pub struct Badge {
    pub text: String,
    pub kind: BadgeKind,
}

#[derive(Debug, Clone)]
pub struct SuperChatInfo {
    #[allow(dead_code)]
    pub amount_display: String,
    #[allow(dead_code)]
    pub tier: u32,
}
