use std::ops::Range;

use annotate_snippets::{AnnotationKind, Level, Renderer, Snippet};

/// Render a lint message as a styled annotate-snippets block.
/// `message` is the display title; `span` is the byte range in `source`
/// to highlight; `path` is the file label shown in the snippet header.
pub fn render_lint(message: &str, span: Range<usize>, source: &str, path: &str) -> String {
    let renderer = Renderer::styled();
    let report = &[Level::WARNING.primary_title(message).element(
        Snippet::source(source)
            .path(path)
            .annotation(AnnotationKind::Primary.span(span).label("")),
    )];
    renderer.render(report)
}
