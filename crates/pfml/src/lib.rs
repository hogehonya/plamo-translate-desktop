//! PFML3 prompt construction for PLaMo 3 Translate.
//!
//! PFML3 delimits prompt elements with special tokens: `<|plamo:tag|>` starts an element,
//! `<|plamo:key|>`/`<|plamo:val|>` attach attributes, and `<|plamo:msg|>` starts its body.
//! The element order follows the official `chat_template.jinja` shipped with the GGUF models:
//!
//! ```text
//! dataset=translation, [context], [history input/output pairs...], [glossary], input, output
//! ```
//!
//! [`Prompt::segments`] returns the prompt as a list of [`Segment`]s so that a tokenizer can
//! map special tokens directly to token ids and tokenize user text *without* special-token
//! parsing. User text that happens to contain `<|plamo:...|>` then cannot inject control
//! tokens. [`Prompt::render`] produces a plain string for servers that accept raw prompts; since
//! the text cannot be escaped there, it rejects reserved token sequences instead.
//!
//! Neither form includes the beginning-of-sequence token; the caller prepends it.

mod lang;

use std::fmt;

use thiserror::Error;

pub use lang::{JA_EASY, LANGUAGES, LangCode, LangCodeError, Language, find_language};

/// Prefix shared by all PFML special tokens.
pub const RESERVED_PREFIX: &str = "<|plamo:";

/// PFML3 special tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecialToken {
    /// Beginning of sequence. The model also emits it to end a translation.
    Bos,
    /// Starts an element. The model emits it after a translation.
    Tag,
    /// Starts an element body.
    Msg,
    /// Starts an attribute name.
    Key,
    /// Starts an attribute value.
    Val,
    /// Starts a glossary source term.
    Index,
    /// Starts a glossary target term.
    Item,
}

impl SpecialToken {
    pub const ALL: [SpecialToken; 7] = [
        Self::Bos,
        Self::Tag,
        Self::Msg,
        Self::Key,
        Self::Val,
        Self::Index,
        Self::Item,
    ];

    /// Tokens whose generation ends a translation.
    pub const STOP: [SpecialToken; 2] = [Self::Tag, Self::Bos];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bos => "<|plamo:bos|>",
            Self::Tag => "<|plamo:tag|>",
            Self::Msg => "<|plamo:msg|>",
            Self::Key => "<|plamo:key|>",
            Self::Val => "<|plamo:val|>",
            Self::Index => "<|plamo:index|>",
            Self::Item => "<|plamo:item|>",
        }
    }
}

/// A piece of a prompt: either a special token or literal text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment<'a> {
    Special(SpecialToken),
    Text(&'a str),
}

/// Japanese output style. Only valid when the target language is `ja`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    /// Plain style (da/dearu, 常体).
    Plain,
    /// Polite style (desu/masu, 敬体).
    Polite,
}

impl Form {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Polite => "polite",
        }
    }
}

/// A fixed translation for a term.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GlossaryEntry {
    pub source: String,
    pub target: String,
}

/// A previously translated source/translation pair, passed to keep wording consistent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HistoryPair {
    pub source: String,
    pub translation: String,
}

/// Request-wide translation options.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TranslationOptions {
    pub target: LangCode,
    /// Source language. `None` lets the model detect it.
    pub source: Option<LangCode>,
    /// Reference context for the whole request. Empty means none.
    pub context: String,
    pub glossary: Vec<GlossaryEntry>,
    pub form: Option<Form>,
}

impl TranslationOptions {
    pub fn new(target: LangCode) -> Self {
        Self {
            target,
            source: None,
            context: String::new(),
            glossary: Vec::new(),
            form: None,
        }
    }
}

/// Identifies the part of a prompt that failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Text,
    Context,
    HistorySource(usize),
    HistoryTranslation(usize),
    GlossarySource(usize),
    GlossaryTarget(usize),
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text => f.write_str("source text"),
            Self::Context => f.write_str("context"),
            Self::HistorySource(i) => write!(f, "history source #{i}"),
            Self::HistoryTranslation(i) => write!(f, "history translation #{i}"),
            Self::GlossarySource(i) => write!(f, "glossary source #{i}"),
            Self::GlossaryTarget(i) => write!(f, "glossary target #{i}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PromptError {
    #[error("{0} must not be empty")]
    Empty(Field),
    #[error("{0} must not contain reserved PFML tokens ({RESERVED_PREFIX}...)")]
    ReservedToken(Field),
    #[error("a style (form) requires the target language to be ja")]
    FormRequiresJapanese,
}

/// One translation prompt: options, optional history, and the text to translate.
#[derive(Debug, Clone, Copy)]
pub struct Prompt<'a> {
    pub options: &'a TranslationOptions,
    /// Earlier pairs in document order.
    pub history: &'a [HistoryPair],
    pub text: &'a str,
}

impl<'a> Prompt<'a> {
    pub fn new(options: &'a TranslationOptions, text: &'a str) -> Self {
        Self {
            options,
            history: &[],
            text,
        }
    }

    pub fn with_history(mut self, history: &'a [HistoryPair]) -> Self {
        self.history = history;
        self
    }

    /// Builds the prompt as segments, without the leading BOS token.
    ///
    /// User text may contain reserved token sequences; the caller must tokenize
    /// [`Segment::Text`] without special-token parsing.
    pub fn segments(&self) -> Result<Vec<Segment<'a>>, PromptError> {
        self.validate()?;
        Ok(self.build())
    }

    /// Renders the prompt as a string, without the leading BOS token.
    ///
    /// Fails if any user-provided text contains a reserved token sequence.
    pub fn render(&self) -> Result<String, PromptError> {
        self.validate()?;
        self.check_reserved()?;
        let mut out = String::new();
        for segment in self.build() {
            match segment {
                Segment::Special(t) => out.push_str(t.as_str()),
                Segment::Text(s) => out.push_str(s),
            }
        }
        Ok(out)
    }

    fn validate(&self) -> Result<(), PromptError> {
        let non_empty = |s: &str, field| {
            if s.trim().is_empty() {
                Err(PromptError::Empty(field))
            } else {
                Ok(())
            }
        };
        non_empty(self.text, Field::Text)?;
        for (i, pair) in self.history.iter().enumerate() {
            non_empty(&pair.source, Field::HistorySource(i))?;
            non_empty(&pair.translation, Field::HistoryTranslation(i))?;
        }
        for (i, entry) in self.options.glossary.iter().enumerate() {
            non_empty(&entry.source, Field::GlossarySource(i))?;
            non_empty(&entry.target, Field::GlossaryTarget(i))?;
        }
        if self.options.form.is_some() && !self.options.target.is_japanese() {
            return Err(PromptError::FormRequiresJapanese);
        }
        Ok(())
    }

    fn check_reserved(&self) -> Result<(), PromptError> {
        let check = |s: &str, field| {
            if s.contains(RESERVED_PREFIX) {
                Err(PromptError::ReservedToken(field))
            } else {
                Ok(())
            }
        };
        check(self.text, Field::Text)?;
        check(&self.options.context, Field::Context)?;
        for (i, pair) in self.history.iter().enumerate() {
            check(&pair.source, Field::HistorySource(i))?;
            check(&pair.translation, Field::HistoryTranslation(i))?;
        }
        for (i, entry) in self.options.glossary.iter().enumerate() {
            check(&entry.source, Field::GlossarySource(i))?;
            check(&entry.target, Field::GlossaryTarget(i))?;
        }
        Ok(())
    }

    fn build(&self) -> Vec<Segment<'a>> {
        use Segment::{Special, Text};
        use SpecialToken::{Index, Item, Key, Msg, Tag, Val};

        let opts = self.options;
        let mut out = Vec::new();
        out.extend([
            Special(Tag),
            Text("dataset"),
            Special(Msg),
            Text("translation"),
        ]);

        if !opts.context.is_empty() {
            out.extend([
                Special(Tag),
                Text("context"),
                Special(Msg),
                Text(&opts.context),
            ]);
        }

        let input = |out: &mut Vec<Segment<'a>>, text: &'a str| {
            out.extend([Special(Tag), Text("input")]);
            if let Some(source) = &opts.source {
                out.extend([
                    Special(Key),
                    Text("lang"),
                    Special(Val),
                    Text(source.as_str()),
                ]);
            }
            out.extend([Special(Msg), Text(text)]);
        };
        let output_header = |out: &mut Vec<Segment<'a>>| {
            out.extend([
                Special(Tag),
                Text("output"),
                Special(Key),
                Text("lang"),
                Special(Val),
                Text(opts.target.as_str()),
            ]);
        };

        for pair in self.history {
            input(&mut out, &pair.source);
            output_header(&mut out);
            out.extend([Special(Msg), Text(&pair.translation)]);
        }

        if !opts.glossary.is_empty() {
            out.extend([Special(Tag), Text("glossary")]);
            for entry in &opts.glossary {
                out.extend([
                    Special(Index),
                    Text(&entry.source),
                    Special(Item),
                    Text(&entry.target),
                ]);
            }
        }

        input(&mut out, self.text);
        output_header(&mut out);
        if let Some(form) = opts.form {
            out.extend([
                Special(Key),
                Text("form"),
                Special(Val),
                Text(form.as_str()),
            ]);
        }
        out.push(Special(Msg));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(s: &str) -> LangCode {
        LangCode::new(s).unwrap()
    }

    fn opts(target: &str) -> TranslationOptions {
        TranslationOptions::new(code(target))
    }

    fn pair(source: &str, translation: &str) -> HistoryPair {
        HistoryPair {
            source: source.into(),
            translation: translation.into(),
        }
    }

    fn term(source: &str, target: &str) -> GlossaryEntry {
        GlossaryEntry {
            source: source.into(),
            target: target.into(),
        }
    }

    // Expected strings below are copied from the PLaMo 3 Translate GGUF model card.

    #[test]
    fn basic() {
        let o = opts("en");
        let p = Prompt::new(&o, "会議は来週の月曜日に変更されました。");
        assert_eq!(
            p.render().unwrap(),
            concat!(
                "<|plamo:tag|>dataset<|plamo:msg|>translation",
                "<|plamo:tag|>input",
                "<|plamo:msg|>会議は来週の月曜日に変更されました。",
                "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>",
            )
        );
    }

    #[test]
    fn history_with_source_lang() {
        let mut o = opts("en");
        o.source = Some(code("ja"));
        let history = [
            pair(
                "プロジェクトの管理画面を開きます。",
                "Open the project dashboard.",
            ),
            pair(
                "進行中のタスクがすべて表示されます。",
                "It lists all active tasks.",
            ),
        ];
        let p =
            Prompt::new(&o, "オーナーでフィルタリングすることもできます。").with_history(&history);
        assert_eq!(
            p.render().unwrap(),
            concat!(
                "<|plamo:tag|>dataset<|plamo:msg|>translation",
                "<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>プロジェクトの管理画面を開きます。",
                "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>Open the project dashboard.",
                "<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>進行中のタスクがすべて表示されます。",
                "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>It lists all active tasks.",
                "<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>オーナーでフィルタリングすることもできます。",
                "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>",
            )
        );
    }

    #[test]
    fn glossary() {
        let mut o = opts("en");
        o.glossary = vec![
            term("管理画面", "dashboard"),
            term("プロジェクト", "project"),
        ];
        let p = Prompt::new(&o, "プロジェクトの管理画面を開いてください。");
        assert_eq!(
            p.render().unwrap(),
            concat!(
                "<|plamo:tag|>dataset<|plamo:msg|>translation",
                "<|plamo:tag|>glossary",
                "<|plamo:index|>管理画面<|plamo:item|>dashboard",
                "<|plamo:index|>プロジェクト<|plamo:item|>project",
                "<|plamo:tag|>input<|plamo:msg|>プロジェクトの管理画面を開いてください。",
                "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>",
            )
        );
    }

    #[test]
    fn context() {
        let mut o = opts("en");
        o.context = "工作で折り紙を作っています。".into();
        let p = Prompt::new(&o, "かみを切りました。");
        assert_eq!(
            p.render().unwrap(),
            concat!(
                "<|plamo:tag|>dataset<|plamo:msg|>translation",
                "<|plamo:tag|>context",
                "<|plamo:msg|>工作で折り紙を作っています。",
                "<|plamo:tag|>input<|plamo:msg|>かみを切りました。",
                "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>",
            )
        );
    }

    #[test]
    fn form() {
        for (form, name) in [(Form::Plain, "plain"), (Form::Polite, "polite")] {
            let mut o = opts("ja");
            o.form = Some(form);
            let p = Prompt::new(&o, "This system is convenient.");
            assert_eq!(
                p.render().unwrap(),
                format!(
                    "<|plamo:tag|>dataset<|plamo:msg|>translation\
                     <|plamo:tag|>input<|plamo:msg|>This system is convenient.\
                     <|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>ja\
                     <|plamo:key|>form<|plamo:val|>{name}<|plamo:msg|>"
                )
            );
        }
    }

    #[test]
    fn all_options() {
        let mut o = opts("en");
        o.source = Some(code("ja"));
        o.context = "プロジェクト管理ツールのヘルプページです。".into();
        o.glossary = vec![term("オーナー", "assignee")];
        let history = [
            pair(
                "プロジェクトの管理画面を開きます。",
                "Open the project dashboard.",
            ),
            pair(
                "進行中のタスクがすべて表示されます。",
                "It lists all active tasks.",
            ),
        ];
        let p =
            Prompt::new(&o, "オーナーでフィルタリングすることもできます。").with_history(&history);
        assert_eq!(
            p.render().unwrap(),
            concat!(
                "<|plamo:tag|>dataset<|plamo:msg|>translation",
                "<|plamo:tag|>context<|plamo:msg|>プロジェクト管理ツールのヘルプページです。",
                "<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>プロジェクトの管理画面を開きます。",
                "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>Open the project dashboard.",
                "<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>進行中のタスクがすべて表示されます。",
                "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>It lists all active tasks.",
                "<|plamo:tag|>glossary<|plamo:index|>オーナー<|plamo:item|>assignee",
                "<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>オーナーでフィルタリングすることもできます。",
                "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>",
            )
        );
    }

    #[test]
    fn segments_keep_user_text_separate() {
        let o = opts("en");
        let text = "literal <|plamo:tag|> in text";
        let segments = Prompt::new(&o, text).segments().unwrap();
        assert!(segments.contains(&Segment::Text(text)));
        assert_eq!(segments.last(), Some(&Segment::Special(SpecialToken::Msg)));
        assert_eq!(
            Prompt::new(&o, text).render(),
            Err(PromptError::ReservedToken(Field::Text))
        );
    }

    #[test]
    fn rejects_empty_text() {
        let o = opts("en");
        assert_eq!(
            Prompt::new(&o, " \n\t").segments(),
            Err(PromptError::Empty(Field::Text))
        );
        let history = [pair("a", " ")];
        assert_eq!(
            Prompt::new(&o, "b").with_history(&history).segments(),
            Err(PromptError::Empty(Field::HistoryTranslation(0)))
        );
        let mut o = opts("en");
        o.glossary = vec![term("", "x")];
        assert_eq!(
            Prompt::new(&o, "b").segments(),
            Err(PromptError::Empty(Field::GlossarySource(0)))
        );
    }

    #[test]
    fn rejects_form_for_non_japanese() {
        for target in ["en", JA_EASY] {
            let mut o = opts(target);
            o.form = Some(Form::Polite);
            assert_eq!(
                Prompt::new(&o, "text").segments(),
                Err(PromptError::FormRequiresJapanese)
            );
        }
    }

    #[test]
    fn ja_easy_target() {
        let o = opts(JA_EASY);
        let rendered = Prompt::new(&o, "Hello.").render().unwrap();
        assert!(rendered.ends_with("<|plamo:val|>ja(easy)<|plamo:msg|>"));
    }
}
