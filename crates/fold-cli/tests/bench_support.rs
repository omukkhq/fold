mod fixture_tests {
    use crate::fixture::*;
    use std::fs;

    #[test]
    fn fixture_is_deterministic() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let sizes = Sizes::smoke();
        let first_total = write_repository(first.path(), sizes.files, sizes.directories).unwrap();
        let second_total = write_repository(second.path(), sizes.files, sizes.directories).unwrap();
        assert_eq!(first_total, second_total);
        for index in 0..sizes.files {
            let path = path_of(index, sizes.directories);
            assert_eq!(
                fs::read(first.path().join(&path)).unwrap(),
                fs::read(second.path().join(&path)).unwrap()
            );
        }
        assert_ne!(content_of(0), content_of(1));
    }

    #[test]
    fn file_sizes_stay_between_4_and_36_kib_of_text() {
        for index in 0..200 {
            let content = content_of(index);
            assert!((4096..=36 * 1024).contains(&content.len()), "{index}");
            assert_eq!(content.last(), Some(&b'\n'));
            assert!(
                content
                    .iter()
                    .all(|byte| byte.is_ascii_hexdigit() || *byte == b'\n')
            );
        }
    }

    #[test]
    fn smoke_divides_every_size_by_100() {
        assert_eq!(
            Sizes::smoke(),
            Sizes {
                files: 100,
                directories: 1,
                proposal_files: 1,
                idle_files: 1
            }
        );
        assert_eq!(path_of(205, 100), "dir005/file00205.txt");
        let full = Sizes::FULL;
        assert_eq!(
            (
                full.files,
                full.directories,
                full.proposal_files,
                full.idle_files
            ),
            (10_000, 100, 50, 100)
        );
    }

    #[test]
    fn edits_at_the_top_and_the_bottom_keep_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "middle\n").unwrap();
        prepend_line(&file, "top").unwrap();
        append_line(&file, "bottom").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "top\nmiddle\nbottom\n");
    }
}

mod report_tests {
    use crate::report::*;
    use crate::targets::Target;

    #[test]
    fn targets_have_a_name_and_a_unit() {
        assert_eq!(Target::IdleMemory.name(), "idle_memory");
        assert_eq!(Target::IdleMemory.unit(), "bytes");
        assert_eq!(Target::Integration.name(), "integration");
        assert_eq!(Target::Integration.unit(), "s");
    }

    #[test]
    fn the_result_file_has_one_entry_per_target_with_every_field() {
        let mut report = Report::default();
        report.record(Target::IdleMemory, 41_000_000.0);
        report.record(Target::WorkspaceCreation, 21.0);
        let parsed: serde_json::Value = serde_json::from_str(&report.json().unwrap()).unwrap();
        let rows = parsed.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[1],
            serde_json::json!({ "target": "workspace_creation", "value": 21.0, "unit": "s" })
        );
        let markdown = report.markdown("host block", "2026-10-15", "0.1.0");
        assert!(markdown.contains("| idle_memory | 41000000 | bytes |"));
        assert!(markdown.contains("host block"));
    }

    #[test]
    fn utc_date_formats_a_unix_time() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_781_481_600), "2026-06-15");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
    }
}
