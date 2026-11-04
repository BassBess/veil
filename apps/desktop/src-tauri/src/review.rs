use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReviewedEntity {
    pub id: String,
    pub entity_type: String,
    pub text: String,
    pub start: usize,
    pub end: usize,
}

fn supported_entity(entity_type: &str) -> bool {
    matches!(
        entity_type,
        "PERSON"
            | "EMAIL_ADDRESS"
            | "PHONE_NUMBER"
            | "LOCATION"
            | "ORGANIZATION"
    )
}

#[tauri::command]
pub fn validate_reviewed_detections(
    text: String,
    detections: Vec<ReviewedEntity>,
) -> Result<Vec<ReviewedEntity>, String> {
    if text.len() > 100_000 {
        return Err("Text exceeds the size limit".into());
    }

    let chars: Vec<char> = text.chars().collect();
    let mut approved = detections;
    let mut ids = HashSet::new();

    for entity in &approved {
        if entity.id.trim().is_empty()
            || !ids.insert(entity.id.clone())
        {
            return Err("Invalid or duplicate detection ID".into());
        }

        if !supported_entity(&entity.entity_type) {
            return Err("Unsupported entity category".into());
        }

        if entity.start >= entity.end || entity.end > chars.len() {
            return Err("Invalid detection position".into());
        }

        let actual: String =
            chars[entity.start..entity.end].iter().collect();

        if actual != entity.text {
            return Err("Detection no longer matches the text".into());
        }
    }

    approved.sort_by_key(|entity| entity.start);

    for pair in approved.windows(2) {
        if pair[0].end > pair[1].start {
            return Err("Approved detections overlap".into());
        }
    }

    Ok(approved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(text: &str, start: usize, end: usize) -> ReviewedEntity {
        ReviewedEntity {
            id: "test-1".into(),
            entity_type: "PERSON".into(),
            text: text.into(),
            start,
            end,
        }
    }

    #[test]
    fn accepts_valid_detection() {
        let result = validate_reviewed_detections(
            "John Smith lives here".into(),
            vec![entity("John Smith", 0, 10)],
        );

        assert!(result.is_ok());
    }

    #[test]
    fn rejects_wrong_text() {
        let result = validate_reviewed_detections(
            "John Smith".into(),
            vec![entity("Jane Smith", 0, 10)],
        );

        assert!(result.is_err());
    }

    #[test]
    fn supports_unicode_positions() {
        let result = validate_reviewed_detections(
            "😀مرحبا".into(),
            vec![entity("مرحبا", 1, 6)],
        );

        assert!(result.is_ok());
    }

    #[test]
    fn rejects_overlapping_detections() {
        let mut second = entity("Smith", 5, 10);
        second.id = "test-2".into();

        let result = validate_reviewed_detections(
            "John Smith".into(),
            vec![entity("John Smith", 0, 10), second],
        );

        assert!(result.is_err());
    }
}

#[cfg(test)]
mod additional_tests {
    use super::*;

    fn entity(id: &str, value: &str, start: usize, end: usize) -> ReviewedEntity {
        ReviewedEntity {
            id: id.into(),
            entity_type: "PERSON".into(),
            text: value.into(),
            start,
            end,
        }
    }

    #[test]
    fn rejects_duplicate_ids() {
        let result = validate_reviewed_detections(
            "John Smith".into(),
            vec![
                entity("same", "John", 0, 4),
                entity("same", "Smith", 5, 10),
            ],
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_unsupported_category() {
        let mut item = entity("1", "John", 0, 4);
        item.entity_type = "UNKNOWN".into();

        assert!(validate_reviewed_detections(
            "John".into(),
            vec![item],
        ).is_err());
    }

    #[test]
    fn rejects_out_of_bounds_positions() {
        assert!(validate_reviewed_detections(
            "John".into(),
            vec![entity("1", "John", 0, 100)],
        ).is_err());
    }

    #[test]
    fn accepts_adjacent_detections() {
        assert!(validate_reviewed_detections(
            "John Smith".into(),
            vec![
                entity("1", "John", 0, 4),
                entity("2", " Smith", 4, 10),
            ],
        ).is_ok());
    }

    #[test]
    fn rejects_empty_detection_id() {
        assert!(validate_reviewed_detections(
            "John".into(),
            vec![entity("", "John", 0, 4)],
        ).is_err());
    }
}
