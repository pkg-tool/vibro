use indexmap::IndexMap;
use serde::Deserialize;
use strum::EnumIter;

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum VsCodeTokenScope {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Deserialize)]
pub struct VsCodeTokenColor {
    pub name: Option<String>,
    pub scope: Option<VsCodeTokenScope>,
    pub settings: VsCodeTokenColorSettings,
}

#[derive(Debug, Deserialize)]
pub struct VsCodeTokenColorSettings {
    pub foreground: Option<String>,
    pub background: Option<String>,
    #[serde(rename = "fontStyle")]
    pub font_style: Option<String>,
}

#[derive(Debug, PartialEq, Copy, Clone, EnumIter)]
pub enum VectorSyntaxToken {
    Attribute,
    Boolean,
    Comment,
    CommentDoc,
    Constant,
    Constructor,
    Embedded,
    Emphasis,
    EmphasisStrong,
    Enum,
    Function,
    Hint,
    Keyword,
    Label,
    LinkText,
    LinkUri,
    Number,
    Operator,
    Predictive,
    Preproc,
    Primary,
    Property,
    Punctuation,
    PunctuationBracket,
    PunctuationDelimiter,
    PunctuationListMarker,
    PunctuationSpecial,
    String,
    StringEscape,
    StringRegex,
    StringSpecial,
    StringSpecialSymbol,
    Tag,
    TextLiteral,
    Title,
    Type,
    Variable,
    VariableSpecial,
    Variant,
}

impl std::fmt::Display for VectorSyntaxToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                VectorSyntaxToken::Attribute => "attribute",
                VectorSyntaxToken::Boolean => "boolean",
                VectorSyntaxToken::Comment => "comment",
                VectorSyntaxToken::CommentDoc => "comment.doc",
                VectorSyntaxToken::Constant => "constant",
                VectorSyntaxToken::Constructor => "constructor",
                VectorSyntaxToken::Embedded => "embedded",
                VectorSyntaxToken::Emphasis => "emphasis",
                VectorSyntaxToken::EmphasisStrong => "emphasis.strong",
                VectorSyntaxToken::Enum => "enum",
                VectorSyntaxToken::Function => "function",
                VectorSyntaxToken::Hint => "hint",
                VectorSyntaxToken::Keyword => "keyword",
                VectorSyntaxToken::Label => "label",
                VectorSyntaxToken::LinkText => "link_text",
                VectorSyntaxToken::LinkUri => "link_uri",
                VectorSyntaxToken::Number => "number",
                VectorSyntaxToken::Operator => "operator",
                VectorSyntaxToken::Predictive => "predictive",
                VectorSyntaxToken::Preproc => "preproc",
                VectorSyntaxToken::Primary => "primary",
                VectorSyntaxToken::Property => "property",
                VectorSyntaxToken::Punctuation => "punctuation",
                VectorSyntaxToken::PunctuationBracket => "punctuation.bracket",
                VectorSyntaxToken::PunctuationDelimiter => "punctuation.delimiter",
                VectorSyntaxToken::PunctuationListMarker => "punctuation.list_marker",
                VectorSyntaxToken::PunctuationSpecial => "punctuation.special",
                VectorSyntaxToken::String => "string",
                VectorSyntaxToken::StringEscape => "string.escape",
                VectorSyntaxToken::StringRegex => "string.regex",
                VectorSyntaxToken::StringSpecial => "string.special",
                VectorSyntaxToken::StringSpecialSymbol => "string.special.symbol",
                VectorSyntaxToken::Tag => "tag",
                VectorSyntaxToken::TextLiteral => "text.literal",
                VectorSyntaxToken::Title => "title",
                VectorSyntaxToken::Type => "type",
                VectorSyntaxToken::Variable => "variable",
                VectorSyntaxToken::VariableSpecial => "variable.special",
                VectorSyntaxToken::Variant => "variant",
            }
        )
    }
}

impl VectorSyntaxToken {
    pub fn find_best_token_color_match<'a>(
        &self,
        token_colors: &'a [VsCodeTokenColor],
    ) -> Option<&'a VsCodeTokenColor> {
        let mut ranked_matches = IndexMap::new();

        for (ix, token_color) in token_colors.iter().enumerate() {
            if token_color.settings.foreground.is_none() {
                continue;
            }

            let Some(rank) = self.rank_match(token_color) else {
                continue;
            };

            if rank > 0 {
                ranked_matches.insert(ix, rank);
            }
        }

        ranked_matches
            .into_iter()
            .max_by_key(|(_, rank)| *rank)
            .map(|(ix, _)| &token_colors[ix])
    }

    fn rank_match(&self, token_color: &VsCodeTokenColor) -> Option<u32> {
        let candidate_scopes = match token_color.scope.as_ref()? {
            VsCodeTokenScope::One(scope) => vec![scope],
            VsCodeTokenScope::Many(scopes) => scopes.iter().collect(),
        }
        .iter()
        .flat_map(|scope| scope.split(',').map(|s| s.trim()))
        .collect::<Vec<_>>();

        let scopes_to_match = self.to_vscode();
        let number_of_scopes_to_match = scopes_to_match.len();

        let mut matches = 0;

        for (ix, scope) in scopes_to_match.into_iter().enumerate() {
            // Assign each entry a weight that is inversely proportional to its
            // position in the list.
            //
            // Entries towards the front are weighted higher than those towards the end.
            let weight = (number_of_scopes_to_match - ix) as u32;

            if candidate_scopes.contains(&scope) {
                matches += 1 + weight;
            }
        }

        Some(matches)
    }

    pub fn fallbacks(&self) -> &[Self] {
        match self {
            VectorSyntaxToken::CommentDoc => &[VectorSyntaxToken::Comment],
            VectorSyntaxToken::Number => &[VectorSyntaxToken::Constant],
            VectorSyntaxToken::VariableSpecial => &[VectorSyntaxToken::Variable],
            VectorSyntaxToken::PunctuationBracket
            | VectorSyntaxToken::PunctuationDelimiter
            | VectorSyntaxToken::PunctuationListMarker
            | VectorSyntaxToken::PunctuationSpecial => &[VectorSyntaxToken::Punctuation],
            VectorSyntaxToken::StringEscape
            | VectorSyntaxToken::StringRegex
            | VectorSyntaxToken::StringSpecial
            | VectorSyntaxToken::StringSpecialSymbol => &[VectorSyntaxToken::String],
            _ => &[],
        }
    }

    fn to_vscode(self) -> Vec<&'static str> {
        match self {
            VectorSyntaxToken::Attribute => vec!["entity.other.attribute-name"],
            VectorSyntaxToken::Boolean => vec!["constant.language"],
            VectorSyntaxToken::Comment => vec!["comment"],
            VectorSyntaxToken::CommentDoc => vec!["comment.block.documentation"],
            VectorSyntaxToken::Constant => {
                vec!["constant", "constant.language", "constant.character"]
            }
            VectorSyntaxToken::Constructor => {
                vec![
                    "entity.name.tag",
                    "entity.name.function.definition.special.constructor",
                ]
            }
            VectorSyntaxToken::Embedded => vec!["meta.embedded"],
            VectorSyntaxToken::Emphasis => vec!["markup.italic"],
            VectorSyntaxToken::EmphasisStrong => vec![
                "markup.bold",
                "markup.italic markup.bold",
                "markup.bold markup.italic",
            ],
            VectorSyntaxToken::Enum => vec!["support.type.enum"],
            VectorSyntaxToken::Function => vec![
                "entity.function",
                "entity.name.function",
                "variable.function",
            ],
            VectorSyntaxToken::Hint => vec![],
            VectorSyntaxToken::Keyword => vec![
                "keyword",
                "keyword.other.fn.rust",
                "keyword.control",
                "keyword.control.fun",
                "keyword.control.class",
                "punctuation.accessor",
                "entity.name.tag",
            ],
            VectorSyntaxToken::Label => vec![
                "label",
                "entity.name",
                "entity.name.import",
                "entity.name.package",
            ],
            VectorSyntaxToken::LinkText => vec!["markup.underline.link", "string.other.link"],
            VectorSyntaxToken::LinkUri => vec!["markup.underline.link", "string.other.link"],
            VectorSyntaxToken::Number => vec!["constant.numeric", "number"],
            VectorSyntaxToken::Operator => vec!["operator", "keyword.operator"],
            VectorSyntaxToken::Predictive => vec![],
            VectorSyntaxToken::Preproc => vec![
                "preproc",
                "meta.preprocessor",
                "punctuation.definition.preprocessor",
            ],
            VectorSyntaxToken::Primary => vec![],
            VectorSyntaxToken::Property => vec![
                "variable.member",
                "support.type.property-name",
                "variable.object.property",
                "variable.other.field",
            ],
            VectorSyntaxToken::Punctuation => vec![
                "punctuation",
                "punctuation.section",
                "punctuation.accessor",
                "punctuation.separator",
                "punctuation.definition.tag",
            ],
            VectorSyntaxToken::PunctuationBracket => vec![
                "punctuation.bracket",
                "punctuation.definition.tag.begin",
                "punctuation.definition.tag.end",
            ],
            VectorSyntaxToken::PunctuationDelimiter => vec![
                "punctuation.delimiter",
                "punctuation.separator",
                "punctuation.terminator",
            ],
            VectorSyntaxToken::PunctuationListMarker => {
                vec!["markup.list punctuation.definition.list.begin"]
            }
            VectorSyntaxToken::PunctuationSpecial => vec!["punctuation.special"],
            VectorSyntaxToken::String => vec!["string"],
            VectorSyntaxToken::StringEscape => {
                vec!["string.escape", "constant.character", "constant.other"]
            }
            VectorSyntaxToken::StringRegex => vec!["string.regex"],
            VectorSyntaxToken::StringSpecial => vec!["string.special", "constant.other.symbol"],
            VectorSyntaxToken::StringSpecialSymbol => {
                vec!["string.special.symbol", "constant.other.symbol"]
            }
            VectorSyntaxToken::Tag => vec!["tag", "entity.name.tag", "meta.tag.sgml"],
            VectorSyntaxToken::TextLiteral => vec!["text.literal", "string"],
            VectorSyntaxToken::Title => vec!["title", "entity.name"],
            VectorSyntaxToken::Type => vec![
                "entity.name.type",
                "entity.name.type.primitive",
                "entity.name.type.numeric",
                "keyword.type",
                "support.type",
                "support.type.primitive",
                "support.class",
            ],
            VectorSyntaxToken::Variable => vec![
                "variable",
                "variable.language",
                "variable.member",
                "variable.parameter",
                "variable.parameter.function-call",
            ],
            VectorSyntaxToken::VariableSpecial => vec![
                "variable.special",
                "variable.member",
                "variable.annotation",
                "variable.language",
            ],
            VectorSyntaxToken::Variant => vec!["variant"],
        }
    }
}
