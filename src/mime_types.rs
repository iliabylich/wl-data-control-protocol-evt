use std::collections::HashSet;

pub(crate) struct MimeTypes {
    mask: String,
}

impl MimeTypes {
    pub(crate) const TEXT: &str = "text/plain";
    pub(crate) const TEXT_UTF8: &str = "text/plain;charset=utf-8";

    pub(crate) fn new() -> Self {
        Self {
            mask: format!(
                "application/x-wayland-clipboard-poll-pid-{}",
                std::process::id()
            ),
        }
    }

    pub(crate) fn mask(&self) -> &str {
        &self.mask
    }

    pub(crate) fn choose(&self, mime_types: HashSet<String>) -> Option<String> {
        if mime_types.contains(&self.mask) {
            None
        } else if mime_types.contains(MimeTypes::TEXT_UTF8) {
            Some(Self::TEXT_UTF8.to_string())
        } else if mime_types.contains(MimeTypes::TEXT) {
            Some(Self::TEXT.to_string())
        } else {
            None
        }
    }

    pub(crate) fn is_text(mime_type: &str) -> bool {
        mime_type == Self::TEXT_UTF8 || mime_type == Self::TEXT
    }
}
