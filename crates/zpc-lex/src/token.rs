//! Token definitions, transcribed from the Pawn compiler's `sc_tokens[]` table
//! (`compiler/libpc300/sc2.c`) so the set matches amxxpc exactly.

use zpc_diag::Span;

/// A lexed token: its kind plus where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// True when this token is the first on its source line. The Pawn grammar is
    /// newline-sensitive in places (an optional-semicolon statement ends at a line
    /// break), so the parser needs this.
    pub line_start: bool,
}

/// Every token kind the Pawn compiler recognises.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    // --- literals and names ---
    /// Integer literal, already converted to a cell value.
    Int(i64),
    /// Rational literal. Pawn stores these as IEEE-754 f32 bit patterns in a cell.
    Rational(f64),
    /// Identifier.
    Ident(String),
    /// `name:` used as a label (distinct from a tag, resolved by the parser).
    Label(String),
    /// String literal contents, escapes already resolved.
    Str(String),
    /// Packed string literal (`!"..."`), stored 4 chars per cell.
    PackedStr(String),

    // --- keywords ---
    Assert, Break, Case, Char, Const, Continue, Default,
    Defined, Do, Else, Enum, Exit, For, Forward, Goto,
    If, Native, New, Operator, Public, Return, Sizeof,
    Sleep, State, Static, Stock, Switch, Tagof, While,

    // --- preprocessor directives (the lexer surfaces them; the preprocessor consumes them) ---
    PpAssert, PpDefine, PpElse, PpElseIf, PpEmit, PpEndIf, PpEndInput,
    PpEndScript, PpError, PpFile, PpIf, PpInclude, PpLine, PpPragma,
    PpTryInclude, PpUndef,

    // --- compound operators (longest match first when lexing) ---
    /// `*=`
    StarAssign,
    /// `/=`
    SlashAssign,
    /// `%=`
    PercentAssign,
    /// `+=`
    PlusAssign,
    /// `-=`
    MinusAssign,
    /// `<<=`
    ShlAssign,
    /// `>>>=`
    UShrAssign,
    /// `>>=`
    ShrAssign,
    /// `&=`
    AndAssign,
    /// `^=`
    XorAssign,
    /// `|=`
    OrAssign,
    /// `||`
    OrOr,
    /// `&&`
    AndAnd,
    /// `==`
    EqEq,
    /// `!=`
    NotEq,
    /// `<=`
    LtEq,
    /// `>=`
    GtEq,
    /// `<<`
    Shl,
    /// `>>>` - logical (zero-filling) right shift
    UShr,
    /// `>>` - arithmetic right shift
    Shr,
    /// `++`
    PlusPlus,
    /// `--`
    MinusMinus,
    /// `...` - rest argument / open-ended array range
    Ellipsis,
    /// `..` - range in a case list
    DotDot,
    /// `::` - state/scope separator
    ColonColon,

    // --- single-character tokens ---
    Plus, Minus, Star, Slash, Percent, Assign, Lt, Gt, Not, Tilde,
    Amp, Pipe, Caret, Question, Colon, Semi, Comma, Dot,
    LParen, RParen, LBrace, RBrace, LBracket, RBracket,

    /// Synthetic token that terminates a pending expression (the compiler's `tENDEXPR`).
    EndExpr,
    /// End of input.
    Eof,
}

impl TokenKind {
    /// The keyword for an identifier, if it is one. Pawn keywords are case-sensitive.
    pub fn keyword(word: &str) -> Option<TokenKind> {
        use TokenKind::*;
        Some(match word {
            "assert" => Assert,
            "break" => Break,
            "case" => Case,
            "char" => Char,
            "const" => Const,
            "continue" => Continue,
            "default" => Default,
            "defined" => Defined,
            "do" => Do,
            "else" => Else,
            "enum" => Enum,
            "exit" => Exit,
            "for" => For,
            "forward" => Forward,
            "goto" => Goto,
            "if" => If,
            "native" => Native,
            "new" => New,
            "operator" => Operator,
            "public" => Public,
            "return" => Return,
            "sizeof" => Sizeof,
            "sleep" => Sleep,
            "state" => State,
            "static" => Static,
            "stock" => Stock,
            "switch" => Switch,
            "tagof" => Tagof,
            "while" => While,
            _ => return None,
        })
    }

    /// The directive for a `#word` spelling, if it is one.
    pub fn directive(word: &str) -> Option<TokenKind> {
        use TokenKind::*;
        Some(match word {
            "assert" => PpAssert,
            "define" => PpDefine,
            "else" => PpElse,
            "elseif" => PpElseIf,
            "emit" => PpEmit,
            "endif" => PpEndIf,
            "endinput" => PpEndInput,
            "endscript" => PpEndScript,
            "error" => PpError,
            "file" => PpFile,
            "if" => PpIf,
            "include" => PpInclude,
            "line" => PpLine,
            "pragma" => PpPragma,
            "tryinclude" => PpTryInclude,
            "undef" => PpUndef,
            _ => return None,
        })
    }

    /// True for the directives that open, continue or close a conditional block.
    /// The preprocessor must track these even while skipping a dead branch.
    pub fn is_conditional_directive(&self) -> bool {
        use TokenKind::*;
        matches!(self, PpIf | PpElseIf | PpElse | PpEndIf)
    }

    /// How the token prints in a diagnostic, matching `sc_tokens[]` spellings.
    pub fn describe(&self) -> &'static str {
        use TokenKind::*;
        match self {
            Int(_) => "-integer value-",
            Rational(_) => "-rational value-",
            Ident(_) => "-identifier-",
            Label(_) => "-label-",
            Str(_) | PackedStr(_) => "-string-",
            StarAssign => "*=", SlashAssign => "/=", PercentAssign => "%=",
            PlusAssign => "+=", MinusAssign => "-=", ShlAssign => "<<=",
            UShrAssign => ">>>=", ShrAssign => ">>=", AndAssign => "&=",
            XorAssign => "^=", OrAssign => "|=", OrOr => "||", AndAnd => "&&",
            EqEq => "==", NotEq => "!=", LtEq => "<=", GtEq => ">=",
            Shl => "<<", UShr => ">>>", Shr => ">>", PlusPlus => "++",
            MinusMinus => "--", Ellipsis => "...", DotDot => "..", ColonColon => "::",
            Assert => "assert", Break => "break", Case => "case", Char => "char",
            Const => "const", Continue => "continue", Default => "default",
            Defined => "defined", Do => "do", Else => "else", Enum => "enum",
            Exit => "exit", For => "for", Forward => "forward", Goto => "goto",
            If => "if", Native => "native", New => "new", Operator => "operator",
            Public => "public", Return => "return", Sizeof => "sizeof",
            Sleep => "sleep", State => "state", Static => "static", Stock => "stock",
            Switch => "switch", Tagof => "tagof", While => "while",
            PpAssert => "#assert", PpDefine => "#define", PpElse => "#else",
            PpElseIf => "#elseif", PpEmit => "#emit", PpEndIf => "#endif",
            PpEndInput => "#endinput", PpEndScript => "#endscript",
            PpError => "#error", PpFile => "#file", PpIf => "#if",
            PpInclude => "#include", PpLine => "#line", PpPragma => "#pragma",
            PpTryInclude => "#tryinclude", PpUndef => "#undef",
            Plus => "+", Minus => "-", Star => "*", Slash => "/", Percent => "%",
            Assign => "=", Lt => "<", Gt => ">", Not => "!", Tilde => "~",
            Amp => "&", Pipe => "|", Caret => "^", Question => "?", Colon => ":",
            Semi => ";", Comma => ",", Dot => ".", LParen => "(", RParen => ")",
            LBrace => "{", RBrace => "}", LBracket => "[", RBracket => "]",
            EndExpr => ";",
            Eof => "-end of file-",
        }
    }
}

/// Multi-character operators, ordered longest-first so that greedy matching picks
/// `>>>=` over `>>>` over `>>`. This ordering is load-bearing - do not sort it.
pub static OPERATORS: &[(&str, TokenKind)] = &[
    (">>>=", TokenKind::UShrAssign),
    ("<<=", TokenKind::ShlAssign),
    (">>=", TokenKind::ShrAssign),
    (">>>", TokenKind::UShr),
    ("...", TokenKind::Ellipsis),
    ("*=", TokenKind::StarAssign),
    ("/=", TokenKind::SlashAssign),
    ("%=", TokenKind::PercentAssign),
    ("+=", TokenKind::PlusAssign),
    ("-=", TokenKind::MinusAssign),
    ("&=", TokenKind::AndAssign),
    ("^=", TokenKind::XorAssign),
    ("|=", TokenKind::OrAssign),
    ("||", TokenKind::OrOr),
    ("&&", TokenKind::AndAnd),
    ("==", TokenKind::EqEq),
    ("!=", TokenKind::NotEq),
    ("<=", TokenKind::LtEq),
    (">=", TokenKind::GtEq),
    ("<<", TokenKind::Shl),
    (">>", TokenKind::Shr),
    ("++", TokenKind::PlusPlus),
    ("--", TokenKind::MinusMinus),
    ("..", TokenKind::DotDot),
    ("::", TokenKind::ColonColon),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operators_are_ordered_longest_first() {
        // Greedy matching takes the first entry that prefixes the input, so a shorter
        // operator must never precede one it is a prefix of (`>>` before `>>>=` would
        // make `>>>=` lex as `>>` `>=`).
        for (later, (a, _)) in OPERATORS.iter().enumerate() {
            for (earlier, _) in &OPERATORS[..later] {
                assert!(
                    !a.starts_with(earlier),
                    "{earlier:?} precedes {a:?}, which it prefixes - greedy matching would break"
                );
            }
        }
    }

    /// The preprocessor hands its control character to the scanner, so the two
    /// defaults must agree. When they disagreed (`\` vs `^`) every `^` escape in
    /// every bundled include failed to lex, and nothing caught it until a real
    /// plugin was compiled.
    #[test]
    fn the_preprocessor_and_scanner_agree_on_the_control_character() {
        assert_eq!(crate::preproc::DEFAULT_CTRLCHAR, crate::scanner::DEFAULT_CTRL_CHAR);
        assert_eq!(crate::scanner::DEFAULT_CTRL_CHAR, b'^');
    }

    #[test]
    fn keyword_set_matches_the_compiler() {
        assert_eq!(TokenKind::keyword("public"), Some(TokenKind::Public));
        assert_eq!(TokenKind::keyword("stock"), Some(TokenKind::Stock));
        // not Pawn keywords, despite looking like them
        assert_eq!(TokenKind::keyword("bool"), None);
        assert_eq!(TokenKind::keyword("float"), None);
        assert_eq!(TokenKind::keyword("Float"), None);
    }

    #[test]
    fn directives_are_recognised_without_the_hash() {
        assert_eq!(TokenKind::directive("include"), Some(TokenKind::PpInclude));
        assert_eq!(TokenKind::directive("tryinclude"), Some(TokenKind::PpTryInclude));
        assert_eq!(TokenKind::directive("nope"), None);
        assert!(TokenKind::PpElseIf.is_conditional_directive());
        assert!(!TokenKind::PpDefine.is_conditional_directive());
    }
}
