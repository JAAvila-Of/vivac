use crate::event::Body;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default, Clone)]
pub(crate) struct Projection {
    revisions: BTreeMap<String, usize>,
    latest: BTreeMap<String, usize>,
    operations: BTreeMap<String, usize>,
    processed: usize,
    invalid: bool,
    missing_producers: BTreeSet<String>,
}

impl Projection {
    pub(crate) fn observe(
        &mut self,
        lane: &str,
        body: &Body,
        history_before: &[(String, Body)],
        producer_exists: bool,
    ) {
        let Some(entry) = super::Revision::from_body(lane, body) else {
            self.invalid = true;
            return;
        };
        let index = history_before.len();
        let stale = self.processed != index;
        self.processed += 1;
        if self.invalid
            || stale
            || super::metadata(&entry).is_err()
            || self.revisions.contains_key(entry.revision)
            || self.operations.contains_key(entry.operation)
        {
            self.invalid = true;
            return;
        }
        let previous = match self.latest.get(entry.artifact) {
            Some(index) => {
                let Some((lane, body)) = history_before.get(*index) else {
                    self.invalid = true;
                    return;
                };
                let Some(previous) = super::Revision::from_body(lane, body) else {
                    self.invalid = true;
                    return;
                };
                if previous.is_bundle() != entry.is_bundle() {
                    self.invalid = true;
                    return;
                }
                Some(previous.revision)
            }
            None => None,
        };
        if previous != entry.previous {
            self.invalid = true;
            return;
        }
        if !producer_exists {
            self.missing_producers.insert(entry.producer.to_owned());
        }
        self.revisions.insert(entry.revision.to_owned(), index);
        self.latest.insert(entry.artifact.to_owned(), index);
        self.operations.insert(entry.operation.to_owned(), index);
    }
    pub(crate) fn resolve_producer(&mut self, id: &str) {
        self.missing_producers.remove(id);
    }
    pub(crate) fn revision_index(&self, id: &str) -> Option<usize> {
        self.revisions.get(id).copied()
    }
    pub(crate) fn latest_index(&self, id: &str) -> Option<usize> {
        self.latest.get(id).copied()
    }
    pub(crate) fn operation_index(&self, id: &str) -> Option<usize> {
        self.operations.get(id).copied()
    }
    pub(crate) fn revisions(&self) -> impl Iterator<Item = (&str, usize)> {
        self.revisions
            .iter()
            .map(|(id, index)| (id.as_str(), *index))
    }
    pub(crate) fn latest(&self) -> impl Iterator<Item = (&str, usize)> {
        self.latest.iter().map(|(id, index)| (id.as_str(), *index))
    }
    pub(crate) fn count(&self) -> usize {
        self.revisions.len()
    }
    #[cfg(test)]
    pub(crate) fn latest_count(&self) -> usize {
        self.latest.len()
    }
    #[cfg(test)]
    pub(crate) fn operations_count(&self) -> usize {
        self.operations.len()
    }
    pub(crate) fn is_valid(&self, history_len: usize) -> bool {
        !self.invalid && self.missing_producers.is_empty() && self.processed == history_len
    }
}
