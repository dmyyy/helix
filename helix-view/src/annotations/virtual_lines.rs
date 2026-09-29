use std::{cell::Cell, sync::Arc};

use helix_core::{
    doc_formatter::FormattedGrapheme,
    text_annotations::LineAnnotation,
    Position,
};

use crate::DocumentId;

/// Immutable per-view reservations for rows following document line endings.
#[derive(Clone, Debug, Default)]
pub struct VirtualLineMap {
    document_version: Option<(DocumentId, i32)>,
    rows: Arc<[(usize, usize)]>,
}

impl VirtualLineMap {
    pub fn new(document: DocumentId, version: i32, mut rows: Vec<(usize, usize)>) -> Self {
        rows.retain(|(_, rows)| *rows != 0);
        rows.sort_unstable_by_key(|&(index, _)| index);
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(rows.len());
        for (index, count) in rows {
            if let Some((last_index, last_count)) = merged.last_mut() {
                if *last_index == index {
                    *last_count = last_count.checked_add(count).expect("virtual row overflow");
                    continue;
                }
            }
            merged.push((index, count));
        }
        Self { document_version: Some((document, version)), rows: merged.into() }
    }

    pub fn matches(&self, document: DocumentId, version: i32) -> bool {
        self.document_version == Some((document, version))
    }

    pub fn rows_at(&self, line_end: usize) -> usize {
        self.rows
            .binary_search_by_key(&line_end, |&(index, _)| index)
            .ok()
            .map(|index| self.rows[index].1)
            .unwrap_or(0)
    }

    pub fn annotation(&self) -> Box<dyn LineAnnotation> {
        Box::new(VirtualLineAnnotation { rows: Arc::clone(&self.rows), next: Cell::new(0), pending: Cell::new(None) })
    }
}

/// Traversal-local state ensures a reservation is consumed only after its real anchor.
struct VirtualLineAnnotation {
    rows: Arc<[(usize, usize)]>,
    next: Cell<usize>,
    pending: Cell<Option<usize>>,
}

impl VirtualLineAnnotation {
    fn next_anchor(&self) -> usize {
        self.rows.get(self.next.get()).map_or(usize::MAX, |&(index, _)| index)
    }
}

impl LineAnnotation for VirtualLineAnnotation {
    fn reset_pos(&mut self, char_idx: usize) -> usize {
        self.next.set(self.rows.partition_point(|&(index, _)| index < char_idx));
        self.pending.set(None);
        self.next_anchor()
    }

    fn skip_concealed_anchors(&mut self, conceal_end_char_idx: usize) -> usize {
        self.next.set(self.rows.partition_point(|&(index, _)| index < conceal_end_char_idx));
        self.pending.set(None);
        self.next_anchor()
    }

    fn process_anchor(&mut self, grapheme: &FormattedGrapheme) -> usize {
        let index = self.next.get();
        if self.rows.get(index).is_some_and(|&(anchor, _)| anchor == grapheme.char_idx) {
            self.pending.set(Some(index));
            self.next.set(index + 1);
        }
        self.next_anchor()
    }

    fn insert_virtual_lines(&mut self, _line_end: usize, _visual_pos: Position, _doc_line: usize) -> Position {
        let Some(index) = self.pending.take() else { return Position::default() };
        Position::new(self.rows[index].1, 0)
    }
}

#[cfg(test)]
mod tests {
    use helix_core::{char_idx_at_visual_offset, doc_formatter::TextFormat, text_annotations::TextAnnotations, visual_offset_from_anchor, Rope};

    use super::*;

    #[test]
    fn virtual_rows_after_text_keep_their_remainder() {
        let text = Rope::from("tag\nnext\n");
        let map = VirtualLineMap::new(DocumentId::default(), 0, vec![(3, 3)]);
        let mut annotations = TextAnnotations::default();
        annotations.add_line_annotation(map.annotation());

        let format = TextFormat::default();
        assert_eq!(
            char_idx_at_visual_offset(text.slice(..), 0, 2, 0, &format, &annotations),
            (3, 2)
        );
        // This is the backward lookup used by j/k viewport correction.
        assert_eq!(
            char_idx_at_visual_offset(text.slice(..), 4, -2, 0, &format, &annotations),
            (3, 2)
        );

        // The image must start below this source newline, not below its reservation.
        assert_eq!(
            visual_offset_from_anchor(text.slice(..), 0, 3, &format, &annotations, usize::MAX),
            Ok((Position::new(0, 3), 0))
        );
    }
}
