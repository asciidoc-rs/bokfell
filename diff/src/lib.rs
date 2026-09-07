//! AsciiDoc-aware structural diff for the Bokfell documentation site
//! generator.
//!
//! The first Bokfell differentiator (PLAN.md §9.1): rendered pages that
//! show what changed between two published versions of a document, or
//! between a pull request's base and head. The engine here is
//! deliberately generator-independent: it takes two sequences of
//! [`BlockUnit`]s — one per document, extracted by the caller in document
//! order — and produces a block-level change model plus word-level
//! `<ins>`/`<del>` HTML for edited blocks. The caller decides how blocks
//! are enumerated (Bokfell uses the same outermost-block walk that backs
//! its coverage overlay, so diff results map 1:1 onto rendered
//! containers).

use similar::{capture_diff_slices, Algorithm, ChangeTag, DiffOp, TextDiff};

/// One diffable block: its resolved context and its source text.
///
/// Two units are *identical* when both fields match; units of the same
/// kind whose texts are similar enough pair up as edited.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockUnit {
    /// The block's resolved context (e.g. `paragraph`, `listing`, `list`).
    pub kind: String,
    /// The block's source text (the span the parser attributes to it).
    pub text: String,
}

/// One aligned change between the old and new block sequences. Indexes
/// refer to the input slices.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockChange {
    /// The block is present in both documents, byte-for-byte.
    Unchanged {
        /// Index in the old sequence.
        old: usize,
        /// Index in the new sequence.
        new: usize,
    },
    /// The block only exists in the new document.
    Added {
        /// Index in the new sequence.
        new: usize,
    },
    /// The block only exists in the old document.
    Removed {
        /// Index in the old sequence.
        old: usize,
    },
    /// The block exists in both documents with modified content.
    Edited {
        /// Index in the old sequence.
        old: usize,
        /// Index in the new sequence.
        new: usize,
        /// The block's source with word-level changes wrapped in
        /// `<del>`/`<ins>` (all text HTML-escaped).
        diff_html: String,
    },
}

/// The diff of two block sequences, in document order of the *new*
/// document (removed blocks are interleaved where their neighbors ended
/// up).
#[derive(Clone, Debug, Default)]
pub struct BlockSeqDiff {
    /// Every aligned change, in order.
    pub changes: Vec<BlockChange>,
    /// Count of added blocks.
    pub added: usize,
    /// Count of removed blocks.
    pub removed: usize,
    /// Count of edited blocks.
    pub edited: usize,
}

impl BlockSeqDiff {
    /// Whether anything changed at all.
    pub fn is_changed(&self) -> bool {
        self.added + self.removed + self.edited > 0
    }

    /// Per-new-block change tokens in new-document order: `Some("added")`
    /// / `Some("edited")` for changed blocks, `None` for unchanged ones.
    /// (Removed blocks have no new-side slot; see
    /// [`removed`](Self::removed).)
    pub fn new_block_tokens(&self, new_len: usize) -> Vec<Option<&'static str>> {
        let mut tokens = vec![None; new_len];
        for change in &self.changes {
            match change {
                BlockChange::Added { new } => tokens[*new] = Some("added"),
                BlockChange::Edited { new, .. } => tokens[*new] = Some("edited"),
                _ => {}
            }
        }
        tokens
    }
}

/// Minimum word-level similarity (0..=1) for a removed/added pair of the
/// same kind to count as one *edited* block rather than two independent
/// changes.
const EDIT_SIMILARITY: f32 = 0.4;

/// Diffs two block sequences: LCS alignment over identical blocks, then
/// similarity pairing of removed/added runs into edits.
pub fn diff_blocks(old: &[BlockUnit], new: &[BlockUnit]) -> BlockSeqDiff {
    let mut diff = BlockSeqDiff::default();

    for op in capture_diff_slices(Algorithm::Patience, old, new) {
        match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                for i in 0..len {
                    diff.changes.push(BlockChange::Unchanged {
                        old: old_index + i,
                        new: new_index + i,
                    });
                }
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                for i in 0..old_len {
                    push_removed(&mut diff, old_index + i);
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for i in 0..new_len {
                    push_added(&mut diff, new_index + i);
                }
            }
            // A replaced run: pair off same-kind blocks in order when
            // they are similar enough, otherwise report them separately.
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let mut old_left = old_index..old_index + old_len;
                for (offset, new_unit) in new[new_index..new_index + new_len].iter().enumerate() {
                    let n = new_index + offset;
                    let paired = old_left.clone().find(|o| {
                        old[*o].kind == new_unit.kind
                            && similarity(&old[*o].text, &new_unit.text) >= EDIT_SIMILARITY
                    });
                    match paired {
                        Some(o) => {
                            // Everything skipped before the pair is gone.
                            for skipped in old_left.start..o {
                                push_removed(&mut diff, skipped);
                            }
                            old_left.start = o + 1;
                            diff.edited += 1;
                            diff.changes.push(BlockChange::Edited {
                                old: o,
                                new: n,
                                diff_html: word_diff_html(&old[o].text, &new_unit.text),
                            });
                        }
                        None => push_added(&mut diff, n),
                    }
                }
                for o in old_left {
                    push_removed(&mut diff, o);
                }
            }
        }
    }

    diff
}

fn push_added(diff: &mut BlockSeqDiff, index: usize) {
    diff.added += 1;
    diff.changes.push(BlockChange::Added { new: index });
}

fn push_removed(diff: &mut BlockSeqDiff, index: usize) {
    diff.removed += 1;
    diff.changes.push(BlockChange::Removed { old: index });
}

/// Word-level similarity of two texts (0..=1).
fn similarity(old: &str, new: &str) -> f32 {
    TextDiff::from_words(old, new).ratio()
}

/// Renders a word-level diff of two source texts as HTML: unchanged text
/// plain, removals in `<del>`, insertions in `<ins>`, everything escaped.
pub fn word_diff_html(old: &str, new: &str) -> String {
    let diff = TextDiff::from_words(old, new);
    let mut out = String::new();
    for change in diff.iter_all_changes() {
        let text = escape_html(change.value());
        match change.tag() {
            ChangeTag::Equal => out.push_str(&text),
            ChangeTag::Delete => {
                out.push_str("<del>");
                out.push_str(&text);
                out.push_str("</del>");
            }
            ChangeTag::Insert => {
                out.push_str("<ins>");
                out.push_str(&text);
                out.push_str("</ins>");
            }
        }
    }
    out
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(kind: &str, text: &str) -> BlockUnit {
        BlockUnit {
            kind: kind.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn identical_sequences_are_unchanged() {
        let blocks = vec![unit("paragraph", "One."), unit("listing", "code")];
        let diff = diff_blocks(&blocks, &blocks);
        assert!(!diff.is_changed());
        assert_eq!(
            diff.changes,
            vec![
                BlockChange::Unchanged { old: 0, new: 0 },
                BlockChange::Unchanged { old: 1, new: 1 },
            ]
        );
        assert_eq!(diff.new_block_tokens(2), vec![None, None]);
    }

    #[test]
    fn additions_removals_and_edits_classify() {
        let old = vec![
            unit("paragraph", "The quick brown fox jumps over the dog."),
            unit("listing", "let x = 1;"),
            unit("paragraph", "Stays the same."),
        ];
        let new = vec![
            unit("paragraph", "The quick red fox jumps over the dog."),
            unit("paragraph", "Stays the same."),
            unit("paragraph", "Entirely new closing thought."),
        ];

        let diff = diff_blocks(&old, &new);
        assert!(diff.is_changed());
        assert_eq!((diff.added, diff.removed, diff.edited), (1, 1, 1));

        // The first paragraph is an edit with word-level markers; the
        // listing is gone; the new closing paragraph is an addition.
        let edited = diff
            .changes
            .iter()
            .find_map(|c| match c {
                BlockChange::Edited { diff_html, .. } => Some(diff_html.clone()),
                _ => None,
            })
            .expect("one edited block");
        assert!(edited.contains("<del>brown</del>"), "html: {edited}");
        assert!(edited.contains("<ins>red</ins>"));
        assert!(diff.changes.contains(&BlockChange::Removed { old: 1 }));
        assert_eq!(
            diff.new_block_tokens(3),
            vec![Some("edited"), None, Some("added")]
        );
    }

    #[test]
    fn dissimilar_replacement_is_add_plus_remove() {
        let old = vec![unit("paragraph", "Alpha beta gamma delta.")];
        let new = vec![unit("paragraph", "Completely unrelated words here now.")];
        let diff = diff_blocks(&old, &new);
        assert_eq!((diff.added, diff.removed, diff.edited), (1, 1, 0));
    }

    #[test]
    fn kind_change_never_pairs_as_edit() {
        let old = vec![unit("paragraph", "some shared words in here")];
        let new = vec![unit("listing", "some shared words in here")];
        let diff = diff_blocks(&old, &new);
        assert_eq!((diff.added, diff.removed, diff.edited), (1, 1, 0));
    }

    #[test]
    fn word_diff_escapes_html() {
        let html = word_diff_html("a <b> c", "a <i> c");
        assert!(html.contains("<del>&lt;b&gt;</del>"));
        assert!(html.contains("<ins>&lt;i&gt;</ins>"));
    }
}
