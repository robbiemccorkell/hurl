use ratatui::text::Text;
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Theme, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

const CACHE_SIZE: usize = 8;
const JSON_THEME_NAME: &str = "base16-ocean.dark";

static JSON_SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static JSON_THEME: LazyLock<Theme> = LazyLock::new(|| {
    let themes = ThemeSet::load_defaults();
    themes
        .themes
        .get(JSON_THEME_NAME)
        .cloned()
        .or_else(|| themes.themes.values().next().cloned())
        .expect("syntect should provide at least one bundled theme")
});
static JSON_CACHE: LazyLock<Mutex<VecDeque<(String, Text<'static>)>>> =
    LazyLock::new(|| Mutex::new(VecDeque::with_capacity(CACHE_SIZE)));

pub fn highlight_json_or_plain(input: &str) -> Text<'static> {
    if let Some(text) = cached(input) {
        return text;
    }

    let highlighted = try_highlight_json(input).unwrap_or_else(|| Text::from(input.to_string()));
    remember(input, &highlighted);
    highlighted
}

fn try_highlight_json(input: &str) -> Option<Text<'static>> {
    let syntax = JSON_SYNTAX_SET.find_syntax_by_extension("json")?;
    let mut highlighter = HighlightLines::new(syntax, &JSON_THEME);
    let mut lines = Vec::new();

    for line in LinesWithEndings::from(input) {
        let spans = highlighter
            .highlight_line(line, &JSON_SYNTAX_SET)
            .ok()?
            .into_iter()
            .map(|(style, segment)| {
                ratatui::text::Span::styled(segment.to_string(), ratatui_style(style))
            })
            .collect::<Vec<_>>();
        lines.push(ratatui::text::Line::from(spans));
    }

    Some(Text::from(lines))
}

fn ratatui_style(style: syntect::highlighting::Style) -> ratatui::style::Style {
    let mut ratatui_style = ratatui::style::Style::default().fg(ratatui::style::Color::Rgb(
        style.foreground.r,
        style.foreground.g,
        style.foreground.b,
    ));

    if style.font_style.contains(FontStyle::BOLD) {
        ratatui_style = ratatui_style.add_modifier(ratatui::style::Modifier::BOLD);
    }

    if style.font_style.contains(FontStyle::ITALIC) {
        ratatui_style = ratatui_style.add_modifier(ratatui::style::Modifier::ITALIC);
    }

    if style.font_style.contains(FontStyle::UNDERLINE) {
        ratatui_style = ratatui_style.add_modifier(ratatui::style::Modifier::UNDERLINED);
    }

    ratatui_style
}

fn cached(input: &str) -> Option<Text<'static>> {
    let mut cache = JSON_CACHE.lock().ok()?;
    let index = cache
        .iter()
        .position(|(cached_input, _)| cached_input == input)?;
    let (cached_input, text) = cache.remove(index)?;
    let result = text.clone();
    cache.push_front((cached_input, text));
    Some(result)
}

fn remember(input: &str, highlighted: &Text<'static>) {
    let Ok(mut cache) = JSON_CACHE.lock() else {
        return;
    };

    if let Some(index) = cache
        .iter()
        .position(|(cached_input, _)| cached_input == input)
    {
        cache.remove(index);
    }

    cache.push_front((input.to_string(), highlighted.clone()));
    while cache.len() > CACHE_SIZE {
        cache.pop_back();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_to_string(text: &Text<'_>) -> String {
        text.lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<String>()
    }

    #[test]
    fn highlights_json_without_changing_content() {
        let input = "{\n  \"ok\": true,\n  \"count\": 2\n}";
        let highlighted = highlight_json_or_plain(input);

        assert_eq!(text_to_string(&highlighted), input);
        assert!(highlighted.lines.iter().any(|line| line.spans.len() > 1));
    }

    #[test]
    fn invalid_json_falls_back_to_plain_text() {
        let input = "{ broken json";
        let highlighted = highlight_json_or_plain(input);

        assert_eq!(text_to_string(&highlighted), input);
    }
}
