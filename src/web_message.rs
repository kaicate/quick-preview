use serde::{Deserialize, Serialize};

use crate::{PreviewError, Result};

pub const MAX_WEB_EDIT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebEditMessage {
    #[serde(rename = "type")]
    pub message_type: String,
    pub document_revision: u64,
    pub node_id: u64,
    pub text: String,
}

impl WebEditMessage {
    pub fn parse(json: &str, expected_revision: u64) -> Result<Self> {
        let message: Self = serde_json::from_str(json)?;
        if message.message_type != "edit" {
            return Err(PreviewError::InvalidWebMessage(
                "unsupported message type".into(),
            ));
        }
        if message.document_revision != expected_revision {
            return Err(PreviewError::InvalidWebMessage(
                "stale document revision".into(),
            ));
        }
        if message.text.len() > MAX_WEB_EDIT_BYTES {
            return Err(PreviewError::InvalidWebMessage("edit is too large".into()));
        }
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_stale_messages() {
        let json = r#"{"type":"edit","documentRevision":2,"nodeId":1,"text":"x"}"#;
        assert!(WebEditMessage::parse(json, 3).is_err());
    }
}
