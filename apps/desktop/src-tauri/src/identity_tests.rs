#[cfg(test)]
mod tests {
    use crate::protection::{SessionVault, find_matches};
    use crate::review::ReviewedEntity;

    #[test]
    fn name_case_and_whitespace_reuse_short_tokens() {
        let mut data = SessionVault::default();
        let first = data.save_identity("PERSON", "John Smith", "").unwrap();
        let same = data
            .save_identity("PERSON", "  john   smith  ", "")
            .unwrap();
        assert_eq!(first.id, same.id);
        assert_eq!(first.pseudonym, "PERSON_001");
        assert_eq!(
            data.save_identity("PERSON", "Jane Doe", "")
                .unwrap()
                .pseudonym,
            "PERSON_002"
        );
        assert_eq!(
            data.save_identity("EMAIL_ADDRESS", "john@example.com", "")
                .unwrap()
                .pseudonym,
            "EMAIL_001"
        );
    }

    #[test]
    fn saved_identity_finds_full_mentions_with_unicode_offsets() {
        let mut data = SessionVault::default();
        let identity = data.save_identity("PERSON", "John", "").unwrap();
        let hits = find_matches(&identity, "\u{1f600} John met JOHN and Johnson").unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!((hits[0].start, hits[0].end), (2, 6));
        assert_eq!(hits[1].text, "JOHN");
        let result = data
            .process(
                "\u{1f600} John met JOHN and Johnson".into(),
                hits,
                "pseudonymize",
            )
            .unwrap();
        assert_eq!(
            result.text,
            "\u{1f600} PERSON_001 met PERSON_001 and Johnson"
        );
    }

    #[test]
    fn existing_literal_label_does_not_collide_with_new_identity() {
        let mut data = SessionVault::default();
        let result = data
            .process(
                "PERSON_001 John".into(),
                vec![ReviewedEntity {
                    id: "1".into(),
                    entity_type: "PERSON".into(),
                    text: "John".into(),
                    start: 11,
                    end: 15,
                }],
                "pseudonymize",
            )
            .unwrap();
        assert_eq!(result.text, "PERSON_001 PERSON_002");
    }
}
