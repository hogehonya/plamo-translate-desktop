//! Glossary text format: one term per line, `source = target`.
//!
//! A tab, `=>` or `→` also separates the two sides. Blank lines and lines starting with `#`
//! are ignored, as are lines without a separator or with an empty side.

use engine::pfml::GlossaryEntry;

const SEPARATORS: [&str; 4] = ["\t", "=>", "→", "="];

pub fn parse(text: &str) -> Vec<GlossaryEntry> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let (source, target) = SEPARATORS.iter().find_map(|sep| line.split_once(sep))?;
            let (source, target) = (source.trim(), target.trim());
            (!source.is_empty() && !target.is_empty()).then(|| GlossaryEntry {
                source: source.to_owned(),
                target: target.to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse;

    fn pairs(text: &str) -> Vec<(String, String)> {
        parse(text)
            .into_iter()
            .map(|e| (e.source, e.target))
            .collect()
    }

    #[test]
    fn parses_supported_separators() {
        let text =
            "管理画面 = dashboard\nオーナー\tassignee\n# comment\n\nタスク => task\n課題→issue\n";
        assert_eq!(
            pairs(text),
            [
                ("管理画面".into(), "dashboard".into()),
                ("オーナー".into(), "assignee".into()),
                ("タスク".into(), "task".into()),
                ("課題".into(), "issue".into()),
            ]
        );
    }

    #[test]
    fn skips_incomplete_lines() {
        assert!(parse("no separator\n = only target\nonly source = \n").is_empty());
    }

    #[test]
    fn splits_at_the_first_separator() {
        assert_eq!(pairs("a = b = c"), [("a".into(), "b = c".into())]);
    }
}
