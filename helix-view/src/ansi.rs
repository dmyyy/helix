use helix_core::SmallVec;
use std::ops::Range;

use crate::graphics::{Color, Modifier, Style, UnderlineStyle};

/// A direct terminal style over a character range in normalized ANSI text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsiStyleSpan {
    pub range: Range<usize>,
    pub style: Style,
}

#[derive(Debug, Default)]
pub struct AnsiRenderOutput {
    pub text: String,
    pub spans: Vec<AnsiStyleSpan>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Face {
    fg: Option<Color>,
    bg: Option<Color>,
    modifiers: Modifier,
    underline: bool,
}

impl Default for Face {
    fn default() -> Self {
        Self {
            fg: None,
            bg: None,
            modifiers: Modifier::empty(),
            underline: false,
        }
    }
}

impl Face {
    fn style(self) -> Style {
        let mut style = Style::reset().underline_style(UnderlineStyle::Reset);
        if let Some(color) = self.fg {
            style = style.fg(color);
        }
        if let Some(color) = self.bg {
            style = style.bg(color);
        }
        if self.underline {
            style = style.underline_style(UnderlineStyle::Line);
        }
        style.add_modifier(self.modifiers)
    }

    fn apply_sgr(&mut self, params: &[u32]) {
        if params.is_empty() {
            *self = Self::default();
            return;
        }

        let mut index = 0;
        while let Some(&param) = params.get(index) {
            index += 1;
            match param {
                0 => *self = Self::default(),
                1 => self.modifiers.insert(Modifier::BOLD),
                2 => self.modifiers.insert(Modifier::DIM),
                3 => self.modifiers.insert(Modifier::ITALIC),
                4 => self.underline = true,
                5 | 6 => self.modifiers.insert(Modifier::SLOW_BLINK),
                7 => self.modifiers.insert(Modifier::REVERSED),
                9 => self.modifiers.insert(Modifier::CROSSED_OUT),
                21 => self.modifiers.remove(Modifier::BOLD),
                22 => self.modifiers.remove(Modifier::BOLD | Modifier::DIM),
                23 => self.modifiers.remove(Modifier::ITALIC),
                24 => self.underline = false,
                25 => self
                    .modifiers
                    .remove(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK),
                27 => self.modifiers.remove(Modifier::REVERSED),
                29 => self.modifiers.remove(Modifier::CROSSED_OUT),
                30..=37 => self.fg = Some(ansi_color(param - 30)),
                38 => self.fg = parse_extended_color(params, &mut index),
                39 => self.fg = None,
                40..=47 => self.bg = Some(ansi_color(param - 40)),
                48 => self.bg = parse_extended_color(params, &mut index),
                49 => self.bg = None,
                90..=97 => self.fg = Some(ansi_color(param - 90 + 8)),
                100..=107 => self.bg = Some(ansi_color(param - 100 + 8)),
                _ => {}
            }
        }
    }
}

fn ansi_color(index: u32) -> Color {
    match index {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::LightGray,
        8 => Color::Gray,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        15 => Color::White,
        _ => unreachable!("ANSI colors have 16 entries"),
    }
}

fn parse_extended_color(params: &[u32], index: &mut usize) -> Option<Color> {
    let selector = *params.get(*index)?;
    *index += 1;
    match selector {
        2 => {
            let (r, g, b) = (
                *params.get(*index)?,
                *params.get(*index + 1)?,
                *params.get(*index + 2)?,
            );
            *index += 3;
            Some(Color::Rgb(
                r.min(255) as u8,
                g.min(255) as u8,
                b.min(255) as u8,
            ))
        }
        5 => {
            let color = *params.get(*index)?;
            *index += 1;
            (color <= u8::MAX.into()).then_some(Color::Indexed(color as u8))
        }
        _ => None,
    }
}

fn line_drawing(ch: char) -> char {
    match ch {
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'q' => '─',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        _ => ch,
    }
}

/// Normalize pager-style ANSI output into text and direct terminal style spans.
///
/// This deliberately ignores terminal cursor movement and erasing controls; it is
/// intended for scrollback and command output, not full-screen terminal programs.
pub fn render(input: &str) -> AnsiRenderOutput {
    let mut output = AnsiRenderOutput {
        text: String::with_capacity(input.len()),
        spans: Vec::new(),
    };
    let mut face = Face::default();
    let mut g1 = false;
    let mut pending: Option<(char, Face)> = None;
    let mut overstrike = false;
    let mut char_count = 0;
    let mut chars = input.chars();

    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => match chars.next() {
                Some('[') => {
                    let mut params = SmallVec::<[u32; 16]>::new();
                    let mut value = None;
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            if let Some(value) = value {
                                if params.len() < 128 {
                                    params.push(value);
                                }
                            }
                            if next == 'm' {
                                face.apply_sgr(&params);
                            }
                            break;
                        }
                        if let Some(digit) = next.to_digit(10) {
                            value = Some(
                                value
                                    .unwrap_or_default()
                                    .saturating_mul(10)
                                    .saturating_add(digit),
                            );
                        } else if let Some(value) = value.take() {
                            if params.len() < 128 {
                                params.push(value);
                            }
                        }
                    }
                }
                Some(']') => {
                    let mut previous = None;
                    for next in chars.by_ref() {
                        if next == '\u{7}' || (previous == Some('\x1b') && next == '\\') {
                            break;
                        }
                        previous = Some(next);
                    }
                }
                Some('(') => {
                    if let Some(charset) = chars.next() {
                        g1 = charset == '0';
                    }
                }
                _ => {}
            },
            '\u{0e}' => g1 = true,
            '\u{0f}' => g1 = false,
            '\r' => {}
            '\u{8}' if pending.is_some() => overstrike = true,
            ch => {
                let ch = if g1 { line_drawing(ch) } else { ch };
                if overstrike {
                    let (previous, mut previous_face) = pending.take().expect("pending overstrike");
                    if previous == ch {
                        previous_face.modifiers.insert(Modifier::BOLD);
                    }
                    if previous == '_' || ch == '_' {
                        previous_face.underline = true;
                    }
                    pending = Some((if ch == '_' { previous } else { ch }, previous_face));
                    overstrike = false;
                } else {
                    flush_pending(&mut output, &mut char_count, &mut pending);
                    pending = Some((ch, face));
                }
            }
        }
    }
    flush_pending(&mut output, &mut char_count, &mut pending);
    output
}

fn flush_pending(
    output: &mut AnsiRenderOutput,
    char_count: &mut usize,
    pending: &mut Option<(char, Face)>,
) {
    if let Some((ch, face)) = pending.take() {
        let style = face.style();
        match output.spans.last_mut() {
            Some(span) if span.style == style && span.range.end == *char_count => {
                span.range.end += 1;
            }
            _ => output.spans.push(AnsiStyleSpan {
                range: *char_count..*char_count + 1,
                style,
            }),
        }
        output.text.push(ch);
        *char_count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_sgr_and_tracks_resets() {
        let output = render("\x1b[31mred\x1b[0m plain");
        assert_eq!(output.text, "red plain");
        assert_eq!(output.spans.len(), 2);
        assert_eq!(output.spans[0].range, 0..3);
        assert_eq!(
            output.spans[0].style,
            Style::reset()
                .underline_style(UnderlineStyle::Reset)
                .fg(Color::Red)
        );
        assert_eq!(output.spans[1].range, 3..9);
        assert_eq!(
            output.spans[1].style,
            Style::reset().underline_style(UnderlineStyle::Reset)
        );
    }

    #[test]
    fn supports_extended_colors_and_attributes() {
        let output = render("\x1b[1;3;38;5;121;48;2;1;2;3mX");
        assert_eq!(output.text, "X");
        assert_eq!(
            output.spans[0].style,
            Style::reset()
                .underline_style(UnderlineStyle::Reset)
                .fg(Color::Indexed(121))
                .bg(Color::Rgb(1, 2, 3))
                .add_modifier(Modifier::BOLD | Modifier::ITALIC)
        );
    }

    #[test]
    fn removes_controls_and_translates_line_drawing() {
        let output = render("a\r\x1b]title\u{7}\x1b(0q\x1b(Bx\x1b[2Kz");
        assert_eq!(output.text, "a─xz");
    }

    #[test]
    fn handles_man_overstrikes() {
        let output = render("X\u{8}X _\u{8}U");
        assert_eq!(output.text, "X U");
        assert_eq!(
            output.spans[0].style,
            Style::reset()
                .underline_style(UnderlineStyle::Reset)
                .add_modifier(Modifier::BOLD)
        );
        assert_eq!(
            output.spans[2].style,
            Style::reset().underline_style(UnderlineStyle::Line)
        );
    }
}
