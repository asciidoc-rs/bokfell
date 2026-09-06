//! Plain-text extraction from inline AST nodes.

use asciidoc_parser::inlines::InlineNode;

/// Collects the plain text of a run of inline nodes: text leaves plus the
/// text inside formatted spans and references. Non-text constructs (images,
/// footnotes, UI macros, …) contribute nothing.
pub(crate) fn inline_text(nodes: &[InlineNode<'_>]) -> String {
    let mut out = String::new();
    collect(nodes, &mut out);
    out
}

fn collect(nodes: &[InlineNode<'_>], out: &mut String) {
    for node in nodes {
        match node {
            InlineNode::Text { value, .. } => out.push_str(value),
            InlineNode::Styled(styled) => collect(&styled.children, out),
            InlineNode::Ref(reference) => collect(&reference.children, out),
            _ => {}
        }
    }
}
