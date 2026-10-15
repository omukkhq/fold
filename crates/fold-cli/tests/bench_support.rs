#[path = "../benches/support/fixture.rs"]
mod fixture;
#[path = "../benches/support/limits.rs"]
mod limits;
#[path = "../benches/support/report.rs"]
mod report;

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

mod limits_tests {
    use crate::limits::*;

    #[test]
    fn limits_scale_with_the_factor() {
        let doubled = Scale::parse(Some("2")).unwrap();
        for (target, name, unit, limit) in BUDGETS {
            assert_eq!(doubled.limit(target), limit * 2.0);
            assert_eq!(Scale::ONE.limit(target), limit);
            assert_eq!(target.name(), name);
            assert_eq!(target.unit(), unit);
        }
        assert_eq!(doubled.factor(), 2.0);
        assert_eq!(Scale::parse(None).unwrap(), Scale::ONE);
        assert_eq!(Scale::parse(Some("1.5")).unwrap().factor(), 1.5);
    }

    #[test]
    fn invalid_scale_is_refused() {
        for value in ["0", "-1", "abc", "", "inf", "NaN"] {
            let error = Scale::parse(Some(value)).unwrap_err();
            assert_eq!(error.code(), "invalid_config", "{value:?}");
            assert!(error.to_string().contains(SCALE_VARIABLE));
        }
    }
}

mod report_tests {
    use crate::limits::{Scale, Target};
    use crate::report::*;

    #[test]
    fn bench_report_marks_a_miss_as_failed() {
        let mut report = Report::default();
        report.record(Target::FullStatScan, 1.5, Scale::ONE);
        assert!(report.all_pass());
        assert_eq!(report.exit_code(true), 0);

        report.record(Target::Integration, 2.5, Scale::ONE);
        assert!(!report.entries()[1].pass);
        assert_eq!(report.exit_code(true), 1);
        assert_eq!(report.exit_code(false), 0, "smoke does not enforce");
    }

    #[test]
    fn a_scaled_limit_passes_what_the_plain_limit_misses() {
        let mut report = Report::default();
        report.record(Target::Integration, 3.0, Scale::parse(Some("2")).unwrap());
        let entry = &report.entries()[0];
        assert!(entry.pass);
        assert_eq!((entry.limit, entry.scale), (4.0, 2.0));
    }

    #[test]
    fn the_result_file_has_one_entry_per_target_with_every_field() {
        let mut report = Report::default();
        report.record(Target::IdleMemory, 41_000_000.0, Scale::ONE);
        report.record(Target::WorkspaceCreation, 21.0, Scale::ONE);
        let parsed: serde_json::Value = serde_json::from_str(&report.json().unwrap()).unwrap();
        let rows = parsed.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[1],
            serde_json::json!({
                "target": "workspace_creation", "value": 21.0, "unit": "s",
                "limit": 20.0, "scale": 1.0, "pass": false
            })
        );
        assert!(report.table().contains("FAIL"));
        let markdown = report.markdown("host block", "2026-10-15", "0.1.0", Scale::ONE);
        assert!(markdown.contains("- Limit scale: 1"));
        assert!(markdown.contains("| idle_memory | 41000000 | bytes | 60000000 | 1 | yes |"));
        assert!(markdown.contains("host block"));
    }

    #[test]
    fn utc_date_formats_a_unix_time() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_781_481_600), "2026-06-15");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
    }
}
