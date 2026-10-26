use fold_core::folder::{UnrecordedKind, unrecorded_changes};
use fold_core::object::{ObjectKind, hash_object};
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use proptest::prelude::*;

const PATHS: [&str; 6] = ["a.txt", "b/c.txt", "b/d", "e", "link", "z.md"];

/// Trees over a few paths with a few contents, so the same path often holds the same entry, another content, another kind, or nothing on each side.
fn tree() -> impl Strategy<Value = FlatTree> {
    let kind = prop_oneof![
        Just(EntryKind::File),
        Just(EntryKind::Executable),
        Just(EntryKind::Symlink),
    ];
    let entry = (kind, 0..3u8).prop_map(|(kind, text)| FlatEntry {
        kind,
        hash: hash_object(ObjectKind::Blob, &[text]),
    });
    prop::collection::vec(prop::option::of(entry), PATHS.len()).prop_map(|entries| {
        PATHS
            .iter()
            .zip(entries)
            .filter_map(|(path, entry)| Some((path.as_bytes().to_vec(), entry?)))
            .collect()
    })
}

proptest! {
    #[test]
    fn the_changes_applied_to_the_record_give_the_folder(record in tree(), folder in tree()) {
        let changes = unrecorded_changes(&record, &folder);
        let mut applied = record.clone();
        for change in &changes {
            match change.kind {
                UnrecordedKind::Deleted => {
                    prop_assert!(applied.remove(&change.path).is_some());
                    prop_assert!(!folder.contains_key(&change.path));
                }
                _ => {
                    let entry = folder.get(&change.path);
                    prop_assert!(entry.is_some());
                    if let Some(entry) = entry {
                        applied.insert(change.path.clone(), *entry);
                    }
                }
            }
        }
        prop_assert_eq!(&applied, &folder);
    }

    #[test]
    fn the_changes_are_in_path_order_once_each_and_named_by_what_differs(record in tree(), folder in tree()) {
        let changes = unrecorded_changes(&record, &folder);
        prop_assert!(changes.windows(2).all(|pair| pair[0].path < pair[1].path));
        for change in &changes {
            let (old, new) = (record.get(&change.path), folder.get(&change.path));
            prop_assert_ne!(old, new);
            let expected = match (old, new) {
                (None, Some(_)) => UnrecordedKind::Added,
                (Some(_), None) => UnrecordedKind::Deleted,
                (Some(old), Some(new))
                    if (old.kind == EntryKind::Symlink) != (new.kind == EntryKind::Symlink) =>
                {
                    UnrecordedKind::TypeChanged
                }
                _ => UnrecordedKind::Modified,
            };
            prop_assert_eq!(change.kind, expected);
            let entry = new.or(old).map(|entry| entry.kind);
            prop_assert_eq!(Some(change.entry), entry);
        }
        let differing = PATHS
            .iter()
            .filter(|path| record.get(path.as_bytes()) != folder.get(path.as_bytes()))
            .count();
        prop_assert_eq!(changes.len(), differing);
    }
}
