use helix_view::ansi::AnsiStyleSpan;
use helix_view::theme::Style;

pub(crate) struct AnsiHighlighter<'s> {
    spans: &'s [AnsiStyleSpan],
    index: usize,
    pub(crate) pos: usize,
    pub(crate) style: Option<Style>,
}

impl<'s> AnsiHighlighter<'s> {
    pub(crate) fn new(spans: &'s [AnsiStyleSpan]) -> Self {
        Self {
            spans,
            index: 0,
            pos: spans.first().map_or(usize::MAX, |span| span.range.start),
            style: None,
        }
    }

    pub(crate) fn advance(&mut self) {
        if let Some(span) = self.spans.get(self.index) {
            self.style = Some(span.style);
            self.pos = span.range.end;
            self.index += 1;
        } else {
            self.style = None;
            self.pos = usize::MAX;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_span_boundaries() {
        let spans = [
            AnsiStyleSpan {
                range: 0..1,
                style: Style::reset().fg(helix_view::graphics::Color::Indexed(1)),
            },
            AnsiStyleSpan {
                range: 1..2,
                style: Style::reset(),
            },
        ];
        let mut highlighter = AnsiHighlighter::new(&spans);

        highlighter.advance();
        assert_eq!(highlighter.pos, 1);
        assert_eq!(highlighter.style, Some(spans[0].style));
        highlighter.advance();
        assert_eq!(highlighter.pos, 2);
        assert_eq!(highlighter.style, Some(spans[1].style));
    }
}
