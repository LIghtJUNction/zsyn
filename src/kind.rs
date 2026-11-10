/// Lossless syntax vocabulary shared by the lexer and concrete syntax tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[repr(u8)]
pub enum SyntaxKind {
    End, Error, Shebang, LineComment, BlockComment,
    Markup, Text, Space, Linebreak, Parbreak, Escape, Shorthand, SmartQuote,
    Strong, Emph, Raw, RawLang, RawDelim, RawTrimmed, Link, Label, Ref, RefMarker,
    Heading, HeadingMarker, ListItem, ListMarker, EnumItem, EnumMarker, TermItem, TermMarker,
    Equation, Math, MathText, MathIdent, MathFieldAccess, MathShorthand, MathAlignPoint,
    MathCall, MathArgs, MathDelimited, MathAttach, MathPrimes, MathFrac, MathRoot,
    Hash, LeftBrace, RightBrace, LeftBracket, RightBracket, LeftParen, RightParen,
    Comma, Semicolon, Colon, Star, Underscore, Dollar, Plus, Minus, Slash, Hat, Dot,
    Eq, EqEq, ExclEq, Lt, LtEq, Gt, GtEq, PlusEq, HyphEq, StarEq, SlashEq, Dots, Arrow,
    Root, Bang, Not, And, Or, None, Auto, Let, Set, Show, Context, If, Else, For, In,
    While, Break, Continue, Return, Import, Include, As,
    Code, Ident, Bool, Int, Float, Numeric, Str, CodeBlock, ContentBlock,
    Parenthesized, Array, Dict, Named, Keyed, Unary, Binary, FieldAccess, FuncCall,
    Args, Spread, Closure, Params, LetBinding, SetRule, ShowRule, Contextual,
    Conditional, WhileLoop, ForLoop, ModuleImport, ImportItems, ImportItemPath,
    RenamedImportItem, ModuleInclude, LoopBreak, LoopContinue, FuncReturn,
    Destructuring, DestructAssignment,
}

impl SyntaxKind {
    /// Whitespace and comments that do not change code expression meaning.
    pub const fn is_trivia(self) -> bool {
        matches!(self, Self::Space | Self::Parbreak | Self::LineComment | Self::BlockComment | Self::Shebang)
    }
    /// Whether this token is an opening or closing grouping delimiter.
    pub const fn is_grouping(self) -> bool {
        matches!(self, Self::LeftParen | Self::RightParen | Self::LeftBrace | Self::RightBrace | Self::LeftBracket | Self::RightBracket)
    }
}

