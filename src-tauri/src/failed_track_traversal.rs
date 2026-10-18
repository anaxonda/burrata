use std::collections::HashSet;

/// Marks the current failed occurrence and returns the next unseen queue entry.
/// Repeat-all wraps once at most; entry IDs keep duplicate track occurrences distinct.
pub(crate) fn next_unfailed_occurrence(
    entries: &[String],
    index: usize,
    repeat: &str,
    failed_occurrence_ids: &mut HashSet<String>,
) -> Result<Option<usize>, ()> {
    if entries.is_empty() || index >= entries.len() {
        return Err(());
    }
    failed_occurrence_ids.insert(entries[index].clone());
    let len = entries.len();
    let mut next = if index + 1 < len {
        index + 1
    } else if repeat == "all" {
        0
    } else {
        return Ok(None);
    };
    for _ in 0..len {
        if !failed_occurrence_ids.contains(&entries[next]) {
            return Ok(Some(next));
        }
        next = if next + 1 < len {
            next + 1
        } else if repeat == "all" {
            0
        } else {
            return Ok(None);
        };
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::next_unfailed_occurrence;
    use std::collections::HashSet;

    #[test]
    fn repeat_all_visits_distinct_duplicate_occurrences_once_then_exhausts() {
        // Queue tracks can share identity; stable entry IDs represent their distinct occurrences.
        let entries = ["entry-a", "entry-b", "entry-c", "entry-d"].map(str::to_owned);
        let mut failed = HashSet::new();
        let mut index = 0;
        let mut visited = Vec::new();
        for expected in [1, 2, 3] {
            visited.push(entries[index].clone());
            let next = next_unfailed_occurrence(&entries, index, "all", &mut failed);
            assert_eq!(next, Ok(Some(expected)));
            index = expected;
        }
        visited.push(entries[index].clone());
        assert_eq!(
            next_unfailed_occurrence(&entries, index, "all", &mut failed),
            Ok(None)
        );
        assert_eq!(visited, entries);
        assert_eq!(failed.len(), entries.len());
    }

    #[test]
    fn repeat_off_exhausts_at_queue_end_without_wrapping() {
        let entries = ["first", "second"].map(str::to_owned);
        let mut failed = HashSet::new();
        assert_eq!(
            next_unfailed_occurrence(&entries, 0, "off", &mut failed),
            Ok(Some(1))
        );
        assert_eq!(
            next_unfailed_occurrence(&entries, 1, "off", &mut failed),
            Ok(None)
        );
    }

    #[test]
    fn a_single_repeat_all_occurrence_cannot_loop() {
        let entries = ["only-entry".to_owned()];
        let mut failed = HashSet::new();
        assert_eq!(
            next_unfailed_occurrence(&entries, 0, "all", &mut failed),
            Ok(None)
        );
    }

    #[test]
    fn malformed_entry_state_is_ineligible_and_does_not_consume_an_occurrence() {
        let entries = ["one".to_owned()];
        let mut failed = HashSet::new();
        assert_eq!(
            next_unfailed_occurrence(&entries, 2, "all", &mut failed),
            Err(())
        );
        assert!(failed.is_empty());
    }
}
