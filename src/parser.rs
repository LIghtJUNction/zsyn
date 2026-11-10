//! Lossless recursive-descent parser with a Pratt expression core.
use crate::{
    Diagnostic, ErrorKind, Lexer, Limits, Mode, Node, ParseError, ParseInfo, Span, SyntaxKind as K,
    Token,
};

const NIL: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct Chain {
    first: u32,
    last: u32,
}
impl Chain {
    const EMPTY: Self = Self {
        first: NIL,
        last: NIL,
    };
}

#[derive(Clone, Copy)]
struct Context {
    newline: bool,
    atomic: bool,
    content: bool,
}
impl Context {
    const CODE: Self = Self {
        newline: false,
        atomic: false,
        content: true,
    };
    const GROUP: Self = Self {
        newline: true,
        atomic: false,
        content: true,
    };
    const EMBED: Self = Self {
        newline: false,
        atomic: true,
        content: true,
    };
    const CONDITION: Self = Self {
        newline: false,
        atomic: false,
        content: false,
    };
}

/// Parses into caller-provided storage and returns the occupied ranges.
pub fn parse_into(
    source: &str,
    mode: Mode,
    nodes: &mut [Node],
    diagnostics: &mut [Diagnostic],
    limits: Limits,
) -> Result<ParseInfo, ParseError> {
    if source.len() > limits.max_source_bytes || source.len() >= u32::MAX as usize {
        return Err(ParseError {
            kind: ErrorKind::SourceLimit,
            span: Span { start: 0, end: 0 },
        });
    }
    let mut p = Parser {
        source,
        nodes,
        diagnostics,
        limits,
        pos: 0,
        used: 0,
        errors: 0,
        depth: 0,
        peak_depth: 0,
        content_depth: 0,
        steps: 0,
        cached: None,
    };
    let root = match mode {
        Mode::Markup => p.markup(None, None, false)?,
        Mode::Code => p.code(None)?,
        Mode::Math => p.math(None, false)?,
    };
    Ok(ParseInfo {
        root,
        nodes: p.used,
        diagnostics: p.errors,
        depth: p.peak_depth,
        steps: p.steps,
        mode,
    })
}

struct Parser<'s, 'a> {
    source: &'s str,
    nodes: &'a mut [Node],
    diagnostics: &'a mut [Diagnostic],
    limits: Limits,
    pos: usize,
    used: usize,
    errors: usize,
    depth: u16,
    peak_depth: u16,
    content_depth: u16,
    steps: usize,
    cached: Option<(Mode, Token)>,
}

impl Parser<'_, '_> {
    fn failure(&self, kind: ErrorKind) -> ParseError {
        ParseError {
            kind,
            span: Span {
                start: self.pos as u32,
                end: self.pos as u32,
            },
        }
    }
    fn tick(&mut self) -> Result<(), ParseError> {
        if self.steps >= self.limits.max_steps {
            return Err(self.failure(ErrorKind::WorkLimit));
        }
        self.steps += 1;
        Ok(())
    }
    fn enter(&mut self) -> Result<(), ParseError> {
        if self.depth >= self.limits.max_depth.min(128) {
            return Err(self.failure(ErrorKind::DepthLimit));
        }
        self.depth += 1;
        self.peak_depth = self.peak_depth.max(self.depth);
        Ok(())
    }
    fn peek(&mut self, mode: Mode) -> Option<Token> {
        if let Some((m, token)) = self.cached {
            if m == mode {
                return Some(token);
            }
        }
        let t = Lexer::at(self.source, mode, self.pos).next();
        self.cached = t.map(|token| (mode, token));
        t
    }
    fn kind(&mut self, mode: Mode) -> K {
        self.peek(mode).map_or(K::End, |t| t.kind)
    }
    fn token_at(&self, mode: Mode, pos: usize) -> Option<Token> {
        Lexer::at(self.source, mode, pos).next()
    }
    fn ahead(&self, mode: Mode) -> (Option<Token>, bool) {
        let mut pos = self.pos;
        let mut newline = false;
        while let Some(t) = self.token_at(mode, pos) {
            if !trivia(t.kind) {
                return (Some(t), newline);
            }
            newline |= self.source[t.span.start as usize..t.span.end as usize]
                .chars()
                .any(is_newline);
            if t.span.end as usize <= pos {
                break;
            }
            pos = t.span.end as usize;
        }
        (None, newline)
    }
    fn report(&mut self, kind: ErrorKind, span: Span) -> Result<(), ParseError> {
        if self.errors >= self.diagnostics.len().min(self.limits.max_diagnostics) {
            return Err(self.failure(ErrorKind::DiagnosticLimit));
        }
        self.diagnostics[self.errors] = Diagnostic { kind, span };
        self.errors += 1;
        Ok(())
    }
    fn node(&mut self, kind: K, span: Span, first_child: u32) -> Result<u32, ParseError> {
        self.tick()?;
        if self.used
            >= self
                .nodes
                .len()
                .min(self.limits.max_nodes)
                .min(u32::MAX as usize)
        {
            return Err(self.failure(ErrorKind::NodeLimit));
        }
        let id = self.used as u32;
        self.nodes[self.used] = Node {
            kind,
            span,
            first_child,
            next_sibling: NIL,
            parent: NIL,
        };
        self.used += 1;
        let mut child = first_child;
        while child != NIL {
            self.nodes[child as usize].parent = id;
            child = self.nodes[child as usize].next_sibling;
        }
        Ok(id)
    }
    fn push(&mut self, chain: &mut Chain, id: u32) {
        if chain.first == NIL {
            chain.first = id;
        } else {
            self.nodes[chain.last as usize].next_sibling = id;
        }
        chain.last = id;
        self.nodes[id as usize].next_sibling = NIL;
    }
    fn single(&mut self, id: u32) -> Chain {
        self.nodes[id as usize].next_sibling = NIL;
        Chain {
            first: id,
            last: id,
        }
    }
    fn group(&mut self, kind: K, chain: Chain, empty: usize) -> Result<u32, ParseError> {
        let span = if chain.first == NIL {
            Span {
                start: empty as u32,
                end: empty as u32,
            }
        } else {
            Span {
                start: self.nodes[chain.first as usize].span.start,
                end: self.nodes[chain.last as usize].span.end,
            }
        };
        self.node(kind, span, chain.first)
    }
    fn bump(&mut self, mode: Mode) -> Result<u32, ParseError> {
        let Some(t) = self.peek(mode) else {
            return self.missing();
        };
        self.pos = t.span.end as usize;
        self.cached = None;
        if let Some(error) = t.error {
            self.report(error, t.span)?;
        }
        self.node(t.kind, t.span, NIL)
    }
    fn bump_as(&mut self, mode: Mode, kind: K) -> Result<u32, ParseError> {
        let id = self.bump(mode)?;
        self.nodes[id as usize].kind = kind;
        Ok(id)
    }
    fn take(&mut self, mode: Mode, chain: &mut Chain) -> Result<u32, ParseError> {
        let id = self.bump(mode)?;
        self.push(chain, id);
        Ok(id)
    }
    fn missing(&mut self) -> Result<u32, ParseError> {
        let span = Span {
            start: self.pos as u32,
            end: self.pos as u32,
        };
        self.report(ErrorKind::ExpectedToken, span)?;
        self.node(K::Error, span, NIL)
    }
    fn expect(&mut self, mode: Mode, kind: K, chain: &mut Chain) -> Result<(), ParseError> {
        let id = if self.kind(mode) == kind {
            self.bump(mode)?
        } else {
            self.missing()?
        };
        self.push(chain, id);
        Ok(())
    }
    fn spaces(&mut self, mode: Mode, chain: &mut Chain) -> Result<(), ParseError> {
        while trivia(self.kind(mode)) {
            self.take(mode, chain)?;
        }
        Ok(())
    }
    fn leading(&mut self, mode: Mode, close: K, chain: &mut Chain) -> Result<(), ParseError> {
        if self.ahead(mode).0.map_or(false, |t| t.kind != close) {
            self.spaces(mode, chain)?;
        }
        Ok(())
    }
    fn until(&mut self, mode: Mode, end: usize, chain: &mut Chain) -> Result<(), ParseError> {
        while self.pos < end {
            self.take(mode, chain)?;
        }
        Ok(())
    }
    fn invalid(&mut self, mode: Mode) -> Result<u32, ParseError> {
        let t = self
            .peek(mode)
            .ok_or_else(|| self.failure(ErrorKind::UnexpectedToken))?;
        self.report(ErrorKind::UnexpectedToken, t.span)?;
        self.bump_as(mode, K::Error)
    }
    fn at_line_start(&self) -> bool {
        self.source[..self.pos]
            .chars()
            .rev()
            .take_while(|&c| !is_newline(c))
            .all(|c| c.is_whitespace())
    }

    fn markup(
        &mut self,
        stop: Option<K>,
        line: Option<usize>,
        term: bool,
    ) -> Result<u32, ParseError> {
        self.enter()?;
        let result = self.markup_inner(stop, line, term);
        self.depth -= 1;
        result
    }
    fn markup_inner(
        &mut self,
        stop: Option<K>,
        line: Option<usize>,
        term: bool,
    ) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut chain = Chain::EMPTY;
        while let Some(t) = self.peek(Mode::Markup) {
            self.tick()?;
            if Some(t.kind) == stop
                || (self.content_depth > 0 && t.kind == K::RightBracket)
                || (term && self.source.as_bytes().get(self.pos) == Some(&b':'))
            {
                break;
            }
            if let Some(indent) = line {
                if trivia(t.kind) {
                    let text = &self.source[t.span.start as usize..t.span.end as usize];
                    if text.chars().any(is_newline) {
                        let continuation =
                            text.chars().rev().take_while(|&c| !is_newline(c)).count();
                        if indent == usize::MAX || continuation < indent {
                            break;
                        }
                    } else if indent == usize::MAX
                        && self.ahead(Mode::Markup).0.map(|next| next.kind) == Some(K::Label)
                    {
                        break;
                    }
                } else if indent == usize::MAX && t.kind == K::Label {
                    break;
                }
            }
            if matches!(stop, Some(K::Star | K::Underscore)) && t.kind == K::Parbreak {
                self.take(Mode::Markup, &mut chain)?;
                break;
            }
            if term && t.kind == K::Text {
                let text = &self.source[t.span.start as usize..t.span.end as usize];
                if let Some(colon) = text.find(':') {
                    if colon > 0 {
                        let end = t.span.start + colon as u32;
                        let id = self.node(
                            K::Text,
                            Span {
                                start: t.span.start,
                                end,
                            },
                            NIL,
                        )?;
                        self.push(&mut chain, id);
                        self.pos = end as usize;
                        self.cached = None;
                    }
                    break;
                }
            }
            let id = match t.kind {
                K::Hash => {
                    self.take(Mode::Markup, &mut chain)?;
                    if self
                        .peek(Mode::Code)
                        .map_or(true, |v| trivia(v.kind) || closing(v.kind))
                    {
                        self.missing()?
                    } else {
                        self.expr(0, Context::EMBED)?
                    }
                }
                K::Dollar => self.equation()?,
                K::Star | K::Underscore => {
                    let mut children = Chain::EMPTY;
                    let opener = self.take(Mode::Markup, &mut children)?;
                    let body = self.markup(Some(t.kind), None, false)?;
                    self.push(&mut children, body);
                    if self.kind(Mode::Markup) == t.kind {
                        self.take(Mode::Markup, &mut children)?;
                    } else {
                        self.nodes[opener as usize].kind = K::Error;
                        self.report(ErrorKind::Unterminated, self.nodes[opener as usize].span)?;
                    }
                    self.group(
                        if t.kind == K::Star {
                            K::Strong
                        } else {
                            K::Emph
                        },
                        children,
                        self.pos,
                    )?
                }
                K::HeadingMarker | K::ListMarker | K::EnumMarker | K::TermMarker
                    if self.at_line_start() =>
                {
                    self.list_heading(t.kind)?
                }
                K::RefMarker => {
                    let mut children = Chain::EMPTY;
                    self.take(Mode::Markup, &mut children)?;
                    if self.kind(Mode::Code) == K::LeftBracket {
                        let content = self.content_block()?;
                        self.push(&mut children, content);
                    }
                    self.group(K::Ref, children, self.pos)?
                }
                K::Raw => self.raw(Mode::Markup)?,
                K::HeadingMarker
                | K::ListMarker
                | K::EnumMarker
                | K::TermMarker
                | K::LeftBracket
                | K::RightBracket
                | K::LeftBrace
                | K::RightBrace
                | K::LeftParen
                | K::RightParen
                | K::Colon => self.bump_as(Mode::Markup, K::Text)?,
                _ => self.bump(Mode::Markup)?,
            };
            self.push(&mut chain, id);
            if t.kind == K::Hash && self.kind(Mode::Code) == K::Semicolon {
                self.take(Mode::Code, &mut chain)?;
            }
        }
        self.group(K::Markup, chain, start)
    }
    fn list_heading(&mut self, marker: K) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        self.take(Mode::Markup, &mut c)?;
        if self.kind(Mode::Markup) == K::Space {
            let t = self.peek(Mode::Markup).unwrap();
            if !self.source[t.span.start as usize..t.span.end as usize]
                .chars()
                .any(is_newline)
            {
                self.take(Mode::Markup, &mut c)?;
            }
        }
        let indent = if marker == K::HeadingMarker {
            usize::MAX
        } else {
            self.source[..self.pos]
                .chars()
                .rev()
                .take_while(|&c| !is_newline(c))
                .count()
        };
        let body = self.markup(None, Some(indent), marker == K::TermMarker)?;
        self.push(&mut c, body);
        if marker == K::TermMarker {
            if self.source.as_bytes().get(self.pos) == Some(&b':') {
                let span = Span {
                    start: self.pos as u32,
                    end: self.pos as u32 + 1,
                };
                self.pos += 1;
                self.cached = None;
                let colon = self.node(K::Colon, span, NIL)?;
                self.push(&mut c, colon);
                if self.kind(Mode::Markup) == K::Space {
                    self.take(Mode::Markup, &mut c)?;
                }
            } else {
                let error = self.missing()?;
                self.push(&mut c, error);
            }
            let body = self.markup(None, Some(indent), false)?;
            self.push(&mut c, body);
        }
        self.group(
            match marker {
                K::HeadingMarker => K::Heading,
                K::ListMarker => K::ListItem,
                K::EnumMarker => K::EnumItem,
                _ => K::TermItem,
            },
            c,
            start,
        )
    }
    fn content_block(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        self.take(Mode::Code, &mut c)?;
        self.content_depth += 1;
        let result = self.markup(Some(K::RightBracket), None, false);
        self.content_depth -= 1;
        let body = result?;
        self.push(&mut c, body);
        self.expect(Mode::Code, K::RightBracket, &mut c)?;
        self.group(K::ContentBlock, c, start)
    }
    fn code_block(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        self.take(Mode::Code, &mut c)?;
        self.leading(Mode::Code, K::RightBrace, &mut c)?;
        let body = self.code(Some(K::RightBrace))?;
        self.push(&mut c, body);
        self.spaces(Mode::Code, &mut c)?;
        self.expect(Mode::Code, K::RightBrace, &mut c)?;
        self.group(K::CodeBlock, c, start)
    }
    fn code(&mut self, stop: Option<K>) -> Result<u32, ParseError> {
        self.enter()?;
        let result = self.code_inner(stop);
        self.depth -= 1;
        result
    }
    fn code_inner(&mut self, stop: Option<K>) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        while self.kind(Mode::Code) != K::End && Some(self.kind(Mode::Code)) != stop {
            self.tick()?;
            let before = self.pos;
            let k = self.kind(Mode::Code);
            if trivia(k)
                && stop.is_some()
                && self
                    .ahead(Mode::Code)
                    .0
                    .map_or(true, |t| Some(t.kind) == stop)
            {
                break;
            }
            let id = if trivia(k) || k == K::Semicolon {
                self.bump(Mode::Code)?
            } else if closing(k) {
                self.invalid(Mode::Code)?
            } else {
                self.expr(0, Context::CODE)?
            };
            self.push(&mut c, id);
            if !trivia(k) && k != K::Semicolon && self.pos != before {
                let (next, newline) = self.ahead(Mode::Code);
                if next.map_or(false, |t| !closing(t.kind) && t.kind != K::Semicolon) && !newline {
                    let err = self.missing()?;
                    self.push(&mut c, err);
                }
            }
            if self.pos == before {
                let id = self.invalid(Mode::Code)?;
                self.push(&mut c, id);
            }
        }
        self.group(K::Code, c, start)
    }
    fn code_value(&mut self, min: u8, ctx: Context, chain: &mut Chain) -> Result<u32, ParseError> {
        if self.ahead(Mode::Code).0.map_or(true, |t| {
            matches!(
                t.kind,
                K::End
                    | K::RightBrace
                    | K::RightBracket
                    | K::RightParen
                    | K::Comma
                    | K::Semicolon
                    | K::Colon
                    | K::Else
            )
        }) {
            return self.missing();
        }
        self.spaces(Mode::Code, chain)?;
        self.expr(min, ctx)
    }
    fn expr(&mut self, min: u8, ctx: Context) -> Result<u32, ParseError> {
        self.enter()?;
        let result = self.expr_inner(min, ctx);
        self.depth -= 1;
        result
    }
    fn expr_inner(&mut self, min: u8, ctx: Context) -> Result<u32, ParseError> {
        let mut lhs = self.primary(ctx)?;
        loop {
            self.tick()?;
            let (next, newline) = self.ahead(Mode::Code);
            let Some(t) = next else {
                break;
            };
            if newline && !ctx.newline {
                break;
            }
            let adjacent = t.span.start as usize == self.pos;
            if t.kind == K::Dot && adjacent {
                if ctx.atomic
                    && self
                        .token_at(Mode::Code, t.span.end as usize)
                        .map(|t| t.kind)
                        != Some(K::Ident)
                {
                    break;
                }
                let mut c = self.single(lhs);
                self.take(Mode::Code, &mut c)?;
                if self.kind(Mode::Code) == K::Ident {
                    self.take(Mode::Code, &mut c)?;
                } else {
                    let e = self.missing()?;
                    self.push(&mut c, e);
                }
                lhs = self.group(K::FieldAccess, c, self.pos)?;
                continue;
            }
            if adjacent && (t.kind == K::LeftParen || (t.kind == K::LeftBracket && ctx.content)) {
                let mut c = self.single(lhs);
                let args = self.arguments()?;
                self.push(&mut c, args);
                lhs = self.group(K::FuncCall, c, self.pos)?;
                continue;
            }
            if ctx.atomic {
                break;
            }
            if t.kind == K::Arrow && min == 0 {
                let lhs_kind = self.nodes[lhs as usize].kind;
                if matches!(
                    lhs_kind,
                    K::Parenthesized | K::Array | K::Dict | K::Destructuring
                ) {
                    self.nodes[lhs as usize].kind = K::Params;
                } else {
                    let c = self.single(lhs);
                    lhs = self.group(K::Params, c, self.pos)?;
                }
                let mut c = self.single(lhs);
                self.until(Mode::Code, t.span.start as usize, &mut c)?;
                self.take(Mode::Code, &mut c)?;
                self.spaces(Mode::Code, &mut c)?;
                let rhs = self.expr(
                    0,
                    Context {
                        atomic: false,
                        ..ctx
                    },
                )?;
                self.push(&mut c, rhs);
                lhs = self.group(K::Closure, c, self.pos)?;
                continue;
            }
            let Some((left, right)) = binding(t.kind) else {
                break;
            };
            if left < min {
                break;
            }
            if t.kind == K::Not {
                let mut after = Lexer::at(self.source, Mode::Code, t.span.end as usize);
                let following = after.find(|v| !trivia(v.kind));
                if following.map(|v| v.kind) != Some(K::In) {
                    break;
                }
            }
            let mut c = self.single(lhs);
            self.until(Mode::Code, t.span.start as usize, &mut c)?;
            self.take(Mode::Code, &mut c)?;
            if t.kind == K::Not {
                self.spaces(Mode::Code, &mut c)?;
                self.expect(Mode::Code, K::In, &mut c)?;
            }
            let rhs = self.code_value(
                right,
                Context {
                    atomic: false,
                    ..ctx
                },
                &mut c,
            )?;
            self.push(&mut c, rhs);
            let kind = if t.kind == K::Eq
                && matches!(
                    self.nodes[lhs as usize].kind,
                    K::Array | K::Dict | K::Parenthesized
                ) {
                self.nodes[lhs as usize].kind = K::Destructuring;
                K::DestructAssignment
            } else {
                K::Binary
            };
            lhs = self.group(kind, c, self.pos)?;
        }
        Ok(lhs)
    }
    fn primary(&mut self, ctx: Context) -> Result<u32, ParseError> {
        let k = self.kind(Mode::Code);
        match k {
            K::Ident
            | K::Bool
            | K::Int
            | K::Float
            | K::Numeric
            | K::Str
            | K::Label
            | K::None
            | K::Auto => self.bump(Mode::Code),
            K::Raw => self.raw(Mode::Code),
            K::LeftParen => self.collection(None),
            K::LeftBrace => self.code_block(),
            K::LeftBracket => self.content_block(),
            K::Dollar => self.equation(),
            K::Plus | K::Minus | K::Not => {
                let start = self.pos;
                let mut c = Chain::EMPTY;
                self.take(Mode::Code, &mut c)?;
                self.spaces(Mode::Code, &mut c)?;
                let rhs = self.expr(
                    if k == K::Not { 5 } else { 8 },
                    Context {
                        atomic: false,
                        ..ctx
                    },
                )?;
                self.push(&mut c, rhs);
                self.group(K::Unary, c, start)
            }
            K::Let => self.let_binding(),
            K::Set => self.set_rule(),
            K::Show => self.show_rule(),
            K::If => self.conditional(),
            K::While => self.while_loop(),
            K::For => self.for_loop(),
            K::Import => self.import(),
            K::Include | K::Context | K::Return => {
                let start = self.pos;
                let mut c = Chain::EMPTY;
                self.take(Mode::Code, &mut c)?;
                let (t, newline) = self.ahead(Mode::Code);
                let can_return =
                    t.map_or(false, |v| !closing(v.kind) && v.kind != K::Semicolon) && !newline;
                if k != K::Return || can_return {
                    self.spaces(Mode::Code, &mut c)?;
                    let rhs = self.expr(0, Context::CODE)?;
                    self.push(&mut c, rhs);
                }
                self.group(
                    match k {
                        K::Include => K::ModuleInclude,
                        K::Context => K::Contextual,
                        _ => K::FuncReturn,
                    },
                    c,
                    start,
                )
            }
            K::Break | K::Continue => {
                let start = self.pos;
                let mut c = Chain::EMPTY;
                self.take(Mode::Code, &mut c)?;
                self.group(
                    if k == K::Break {
                        K::LoopBreak
                    } else {
                        K::LoopContinue
                    },
                    c,
                    start,
                )
            }
            K::End
            | K::RightBrace
            | K::RightBracket
            | K::RightParen
            | K::Comma
            | K::Semicolon
            | K::Colon
            | K::Else => self.missing(),
            K::Error => self.bump(Mode::Code),
            _ => self.invalid(Mode::Code),
        }
    }
    fn arguments(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        if self.kind(Mode::Code) == K::LeftParen {
            self.sequence(&mut c, K::RightParen, false)?;
        }
        while self.kind(Mode::Code) == K::LeftBracket {
            let body = self.content_block()?;
            self.push(&mut c, body);
        }
        self.group(K::Args, c, start)
    }
    fn collection(&mut self, fixed: Option<K>) -> Result<u32, ParseError> {
        self.enter()?;
        let result = self.collection_inner(fixed);
        self.depth -= 1;
        result
    }
    fn collection_inner(&mut self, fixed: Option<K>) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        let (items, commas, named) = self.sequence(
            &mut c,
            K::RightParen,
            fixed == Some(K::Destructuring) || fixed == Some(K::Params),
        )?;
        let kind = fixed.unwrap_or(if named {
            K::Dict
        } else if commas || items == 0 {
            K::Array
        } else {
            K::Parenthesized
        });
        self.group(kind, c, start)
    }
    fn sequence(
        &mut self,
        c: &mut Chain,
        close: K,
        pattern: bool,
    ) -> Result<(usize, bool, bool), ParseError> {
        self.take(Mode::Code, c)?;
        let mut items = 0;
        let mut commas = false;
        let mut named = false;
        loop {
            self.tick()?;
            self.spaces(Mode::Code, c)?;
            if matches!(self.kind(Mode::Code), K::End) || self.kind(Mode::Code) == close {
                break;
            }
            if self.kind(Mode::Code) == K::Colon && items == 0 {
                named = true;
                self.take(Mode::Code, c)?;
                continue;
            }
            if closing(self.kind(Mode::Code)) {
                break;
            }
            let before = self.pos;
            let mut id = if self.kind(Mode::Code) == K::Dots {
                let start = self.pos;
                let mut spread = Chain::EMPTY;
                self.take(Mode::Code, &mut spread)?;
                self.spaces(Mode::Code, &mut spread)?;
                if !matches!(self.kind(Mode::Code), K::Comma | K::RightParen | K::End) {
                    let value = if pattern {
                        self.pattern()?
                    } else {
                        self.expr(0, Context::GROUP)?
                    };
                    self.push(&mut spread, value);
                }
                self.group(K::Spread, spread, start)?
            } else if pattern {
                self.pattern()?
            } else {
                self.expr(0, Context::GROUP)?
            };
            let (next, _) = self.ahead(Mode::Code);
            if let Some(t) = next.filter(|t| t.kind == K::Colon) {
                named = true;
                let kind = if self.nodes[id as usize].kind == K::Ident {
                    K::Named
                } else {
                    K::Keyed
                };
                let mut pair = self.single(id);
                self.until(Mode::Code, t.span.start as usize, &mut pair)?;
                self.take(Mode::Code, &mut pair)?;
                let value = self.code_value(0, Context::GROUP, &mut pair)?;
                self.push(&mut pair, value);
                id = self.group(kind, pair, self.pos)?;
            }
            self.push(c, id);
            items += 1;
            self.spaces(Mode::Code, c)?;
            if self.kind(Mode::Code) == K::Comma {
                commas = true;
                self.take(Mode::Code, c)?;
            } else if self.kind(Mode::Code) != close && self.kind(Mode::Code) != K::End {
                let err = self.missing()?;
                self.push(c, err);
                if self.pos == before {
                    let bad = self.invalid(Mode::Code)?;
                    self.push(c, bad);
                }
            }
        }
        self.expect(Mode::Code, close, c)?;
        Ok((items, commas, named))
    }
    fn pattern(&mut self) -> Result<u32, ParseError> {
        match self.kind(Mode::Code) {
            K::LeftParen => self.collection(Some(K::Destructuring)),
            K::Ident | K::Underscore => self.bump(Mode::Code),
            _ => self.primary(Context::GROUP),
        }
    }
    fn let_binding(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        self.take(Mode::Code, &mut c)?;
        self.spaces(Mode::Code, &mut c)?;
        let pattern = self.pattern()?;
        if self.nodes[pattern as usize].kind == K::Ident && self.kind(Mode::Code) == K::LeftParen {
            let mut closure = self.single(pattern);
            let params = self.collection(Some(K::Params))?;
            self.push(&mut closure, params);
            self.spaces(Mode::Code, &mut closure)?;
            self.expect(Mode::Code, K::Eq, &mut closure)?;
            self.spaces(Mode::Code, &mut closure)?;
            let value = self.expr(0, Context::CODE)?;
            self.push(&mut closure, value);
            let node = self.group(K::Closure, closure, start)?;
            self.push(&mut c, node);
        } else {
            self.push(&mut c, pattern);
            let (next, newline) = self.ahead(Mode::Code);
            if let Some(t) = next.filter(|t| t.kind == K::Eq && !newline) {
                self.until(Mode::Code, t.span.start as usize, &mut c)?;
                self.take(Mode::Code, &mut c)?;
                self.spaces(Mode::Code, &mut c)?;
                let value = self.expr(0, Context::CODE)?;
                self.push(&mut c, value);
            }
        }
        self.group(K::LetBinding, c, start)
    }
    fn set_rule(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        self.take(Mode::Code, &mut c)?;
        self.spaces(Mode::Code, &mut c)?;
        let mut target = if self.kind(Mode::Code) == K::Ident {
            self.bump(Mode::Code)?
        } else {
            self.missing()?
        };
        while self.kind(Mode::Code) == K::Dot {
            let mut field = self.single(target);
            self.take(Mode::Code, &mut field)?;
            self.expect(Mode::Code, K::Ident, &mut field)?;
            target = self.group(K::FieldAccess, field, start)?;
        }
        self.push(&mut c, target);
        if self.kind(Mode::Code) == K::LeftParen {
            let args = self.arguments()?;
            self.push(&mut c, args);
        } else {
            let err = self.missing()?;
            self.push(&mut c, err);
        }
        let (next, newline) = self.ahead(Mode::Code);
        if let Some(t) = next.filter(|t| t.kind == K::If && !newline) {
            self.until(Mode::Code, t.span.start as usize, &mut c)?;
            self.take(Mode::Code, &mut c)?;
            self.spaces(Mode::Code, &mut c)?;
            let cond = self.expr(0, Context::CODE)?;
            self.push(&mut c, cond);
        }
        self.group(K::SetRule, c, start)
    }
    fn show_rule(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        self.take(Mode::Code, &mut c)?;
        self.spaces(Mode::Code, &mut c)?;
        if self.kind(Mode::Code) != K::Colon {
            let selector = self.expr(0, Context::CODE)?;
            self.push(&mut c, selector);
            self.spaces(Mode::Code, &mut c)?;
        }
        self.expect(Mode::Code, K::Colon, &mut c)?;
        self.spaces(Mode::Code, &mut c)?;
        let transform = self.expr(0, Context::CODE)?;
        self.push(&mut c, transform);
        self.group(K::ShowRule, c, start)
    }
    fn body(&mut self) -> Result<u32, ParseError> {
        match self.kind(Mode::Code) {
            K::LeftBrace => self.code_block(),
            K::LeftBracket => self.content_block(),
            _ => self.missing(),
        }
    }
    fn conditional(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        self.take(Mode::Code, &mut c)?;
        self.spaces(Mode::Code, &mut c)?;
        let condition = self.expr(0, Context::CONDITION)?;
        self.push(&mut c, condition);
        self.spaces(Mode::Code, &mut c)?;
        let body = self.body()?;
        self.push(&mut c, body);
        let (next, _) = self.ahead(Mode::Code);
        if let Some(t) = next.filter(|t| t.kind == K::Else) {
            self.until(Mode::Code, t.span.start as usize, &mut c)?;
            self.take(Mode::Code, &mut c)?;
            self.spaces(Mode::Code, &mut c)?;
            let other = if self.kind(Mode::Code) == K::If {
                self.enter()?;
                let r = self.conditional();
                self.depth -= 1;
                r?
            } else {
                self.body()?
            };
            self.push(&mut c, other);
        }
        self.group(K::Conditional, c, start)
    }
    fn while_loop(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        self.take(Mode::Code, &mut c)?;
        self.spaces(Mode::Code, &mut c)?;
        let condition = self.expr(0, Context::CONDITION)?;
        self.push(&mut c, condition);
        self.spaces(Mode::Code, &mut c)?;
        let body = self.body()?;
        self.push(&mut c, body);
        self.group(K::WhileLoop, c, start)
    }
    fn for_loop(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        self.take(Mode::Code, &mut c)?;
        self.spaces(Mode::Code, &mut c)?;
        let pattern = self.pattern()?;
        self.push(&mut c, pattern);
        self.spaces(Mode::Code, &mut c)?;
        if self.kind(Mode::Code) == K::Comma {
            self.take(Mode::Code, &mut c)?;
            self.spaces(Mode::Code, &mut c)?;
            let second = self.pattern()?;
            self.push(&mut c, second);
            self.spaces(Mode::Code, &mut c)?;
        }
        self.expect(Mode::Code, K::In, &mut c)?;
        self.spaces(Mode::Code, &mut c)?;
        let collection = self.expr(6, Context::CONDITION)?;
        self.push(&mut c, collection);
        self.spaces(Mode::Code, &mut c)?;
        let body = self.body()?;
        self.push(&mut c, body);
        self.group(K::ForLoop, c, start)
    }
    fn import(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        self.take(Mode::Code, &mut c)?;
        self.spaces(Mode::Code, &mut c)?;
        let source = self.expr(0, Context::CODE)?;
        self.push(&mut c, source);
        let (next, newline) = self.ahead(Mode::Code);
        if let Some(t) = next.filter(|t| matches!(t.kind, K::As | K::Colon) && !newline) {
            self.until(Mode::Code, t.span.start as usize, &mut c)?;
            self.take(Mode::Code, &mut c)?;
            self.spaces(Mode::Code, &mut c)?;
            if t.kind == K::As {
                self.expect(Mode::Code, K::Ident, &mut c)?;
            } else if self.kind(Mode::Code) == K::Star {
                self.take(Mode::Code, &mut c)?;
            } else {
                let mut items = Chain::EMPTY;
                loop {
                    let item_start = self.pos;
                    let mut path = Chain::EMPTY;
                    self.expect(Mode::Code, K::Ident, &mut path)?;
                    while self.kind(Mode::Code) == K::Dot {
                        self.take(Mode::Code, &mut path)?;
                        self.expect(Mode::Code, K::Ident, &mut path)?;
                    }
                    let mut item = self.group(K::ImportItemPath, path, item_start)?;
                    let (next, newline) = self.ahead(Mode::Code);
                    if let Some(t) = next.filter(|t| t.kind == K::As && !newline) {
                        let mut rename = self.single(item);
                        self.until(Mode::Code, t.span.start as usize, &mut rename)?;
                        self.take(Mode::Code, &mut rename)?;
                        self.spaces(Mode::Code, &mut rename)?;
                        self.expect(Mode::Code, K::Ident, &mut rename)?;
                        item = self.group(K::RenamedImportItem, rename, item_start)?;
                    }
                    self.push(&mut items, item);
                    let (next, newline) = self.ahead(Mode::Code);
                    if let Some(t) = next.filter(|t| t.kind == K::Comma && !newline) {
                        self.until(Mode::Code, t.span.start as usize, &mut items)?;
                        self.take(Mode::Code, &mut items)?;
                        self.spaces(Mode::Code, &mut items)?;
                    } else {
                        break;
                    }
                    if self.kind(Mode::Code) == K::End || closing(self.kind(Mode::Code)) {
                        break;
                    }
                }
                let items = self.group(K::ImportItems, items, self.pos)?;
                self.push(&mut c, items);
            }
        }
        self.group(K::ModuleImport, c, start)
    }
    fn equation(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        self.take(Mode::Math, &mut c)?;
        self.leading(Mode::Math, K::Dollar, &mut c)?;
        let body = self.math(Some(K::Dollar), false)?;
        self.push(&mut c, body);
        self.spaces(Mode::Math, &mut c)?;
        self.expect(Mode::Math, K::Dollar, &mut c)?;
        self.group(K::Equation, c, start)
    }
    fn math(&mut self, stop: Option<K>, arguments: bool) -> Result<u32, ParseError> {
        self.enter()?;
        let result = self.math_inner(stop, arguments);
        self.depth -= 1;
        result
    }
    fn math_inner(&mut self, stop: Option<K>, arguments: bool) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        while let Some(t) = self.peek(Mode::Math) {
            self.tick()?;
            if Some(t.kind) == stop || (arguments && matches!(t.kind, K::Comma | K::Semicolon)) {
                break;
            }
            if stop.is_some()
                && trivia(t.kind)
                && self.ahead(Mode::Math).0.map_or(true, |next| {
                    Some(next.kind) == stop || next.kind == K::Dollar
                })
            {
                break;
            }
            if stop.is_some() && t.kind == K::Dollar {
                break;
            }
            let before = self.pos;
            if t.kind == K::Hash {
                self.take(Mode::Math, &mut c)?;
                let expr = if self
                    .peek(Mode::Code)
                    .map_or(true, |v| trivia(v.kind) || closing(v.kind))
                {
                    self.missing()?
                } else {
                    self.expr(0, Context::EMBED)?
                };
                self.push(&mut c, expr);
            } else {
                let id = if trivia(t.kind) {
                    self.bump(Mode::Math)?
                } else {
                    self.math_expr(0, None)?
                };
                self.push(&mut c, id);
            }
            if self.pos == before {
                let id = self.invalid(Mode::Math)?;
                self.push(&mut c, id);
            }
        }
        self.group(K::Math, c, start)
    }
    fn math_expr(&mut self, min: u8, stop_attach: Option<K>) -> Result<u32, ParseError> {
        self.enter()?;
        let result = self.math_expr_inner(min, stop_attach);
        self.depth -= 1;
        result
    }
    fn math_expr_inner(&mut self, min: u8, stop_attach: Option<K>) -> Result<u32, ParseError> {
        let mut lhs = self.math_primary()?;
        let mut first_attach = None;
        loop {
            self.tick()?;
            let (next, _) = self.ahead(Mode::Math);
            let Some(t) = next else {
                break;
            };
            let adjacent = t.span.start as usize == self.pos;
            let lhs_kind = self.nodes[lhs as usize].kind;
            if adjacent && t.kind == K::Dot && matches!(lhs_kind, K::MathIdent | K::MathFieldAccess)
            {
                let mut c = self.single(lhs);
                self.take(Mode::Math, &mut c)?;
                if matches!(self.kind(Mode::Math), K::MathIdent | K::MathText) {
                    let field = self.bump_as(Mode::Math, K::MathIdent)?;
                    self.push(&mut c, field);
                } else {
                    let err = self.missing()?;
                    self.push(&mut c, err);
                }
                lhs = self.group(K::MathFieldAccess, c, self.pos)?;
                continue;
            }
            if adjacent
                && t.kind == K::LeftParen
                && matches!(lhs_kind, K::MathIdent | K::MathFieldAccess)
            {
                let mut c = self.single(lhs);
                let args = self.math_args()?;
                self.push(&mut c, args);
                lhs = self.group(K::MathCall, c, self.pos)?;
                continue;
            }
            if adjacent
                && matches!(
                    t.kind,
                    K::Bang | K::LeftParen | K::LeftBracket | K::LeftBrace
                )
                && min < 4
            {
                let mut c = self.single(lhs);
                let rhs = if t.kind == K::Bang {
                    self.bump_as(Mode::Math, K::MathText)?
                } else {
                    self.math_primary()?
                };
                self.push(&mut c, rhs);
                lhs = self.group(K::Math, c, self.pos)?;
                continue;
            }
            if t.kind == K::MathPrimes && adjacent {
                let mut c = self.single(lhs);
                self.take(Mode::Math, &mut c)?;
                lhs = self.group(K::MathAttach, c, self.pos)?;
                first_attach = Some(K::MathPrimes);
                continue;
            }
            if Some(t.kind) == stop_attach {
                break;
            }
            if matches!(t.kind, K::Underscore | K::Hat) && min <= 3 {
                let append = first_attach.is_some()
                    && first_attach != Some(t.kind)
                    && lhs_kind == K::MathAttach;
                let mut c = if append {
                    let first = self.nodes[lhs as usize].first_child;
                    let mut last = first;
                    while self.nodes[last as usize].next_sibling != NIL {
                        last = self.nodes[last as usize].next_sibling;
                    }
                    Chain { first, last }
                } else {
                    self.single(lhs)
                };
                self.until(Mode::Math, t.span.start as usize, &mut c)?;
                self.take(Mode::Math, &mut c)?;
                let rhs = if self.ahead(Mode::Math).0.map_or(true, |v| {
                    matches!(
                        v.kind,
                        K::Dollar
                            | K::RightParen
                            | K::RightBracket
                            | K::RightBrace
                            | K::Comma
                            | K::Semicolon
                    )
                }) {
                    self.missing()?
                } else {
                    self.spaces(Mode::Math, &mut c)?;
                    self.math_expr(
                        3,
                        Some(if t.kind == K::Hat {
                            K::Underscore
                        } else {
                            K::Hat
                        }),
                    )?
                };
                self.unparen(rhs);
                self.push(&mut c, rhs);
                if append {
                    self.nodes[lhs as usize].span.end = self.nodes[c.last as usize].span.end;
                    let mut child = c.first;
                    while child != NIL {
                        self.nodes[child as usize].parent = lhs;
                        child = self.nodes[child as usize].next_sibling;
                    }
                } else {
                    lhs = self.group(K::MathAttach, c, self.pos)?;
                }
                first_attach = Some(t.kind);
                continue;
            }
            if t.kind == K::Slash && min <= 2 {
                self.unparen(lhs);
                let mut c = self.single(lhs);
                self.until(Mode::Math, t.span.start as usize, &mut c)?;
                self.take(Mode::Math, &mut c)?;
                self.spaces(Mode::Math, &mut c)?;
                let rhs = self.math_expr(3, None)?;
                self.unparen(rhs);
                self.push(&mut c, rhs);
                lhs = self.group(K::MathFrac, c, self.pos)?;
                first_attach = None;
                continue;
            }
            break;
        }
        Ok(lhs)
    }
    fn unparen(&mut self, id: u32) {
        if self.nodes[id as usize].kind != K::MathDelimited {
            return;
        }
        let first = self.nodes[id as usize].first_child;
        if first == NIL {
            return;
        }
        let span = self.nodes[first as usize].span;
        if &self.source[span.start as usize..span.end as usize] != "(" {
            return;
        }
        let mut last = first;
        while self.nodes[last as usize].next_sibling != NIL {
            last = self.nodes[last as usize].next_sibling;
        }
        self.nodes[id as usize].kind = K::Math;
        self.nodes[first as usize].kind = K::LeftParen;
        if self.nodes[last as usize].kind == K::MathText {
            self.nodes[last as usize].kind = K::RightParen;
        }
    }
    fn math_primary(&mut self) -> Result<u32, ParseError> {
        match self.kind(Mode::Math) {
            K::LeftParen | K::LeftBracket | K::LeftBrace => {
                let start = self.pos;
                let open = self.kind(Mode::Math);
                let close = match open {
                    K::LeftParen => K::RightParen,
                    K::LeftBracket => K::RightBracket,
                    _ => K::RightBrace,
                };
                let mut c = Chain::EMPTY;
                let id = self.bump_as(Mode::Math, K::MathText)?;
                self.push(&mut c, id);
                self.leading(Mode::Math, close, &mut c)?;
                let body = self.math(Some(close), false)?;
                self.push(&mut c, body);
                self.spaces(Mode::Math, &mut c)?;
                if self.kind(Mode::Math) == close {
                    let id = self.bump_as(Mode::Math, K::MathText)?;
                    self.push(&mut c, id);
                } else {
                    let err = self.missing()?;
                    self.push(&mut c, err);
                }
                self.group(K::MathDelimited, c, start)
            }
            K::Root => {
                let start = self.pos;
                let mut c = Chain::EMPTY;
                self.take(Mode::Math, &mut c)?;
                self.spaces(Mode::Math, &mut c)?;
                let value = self.math_expr(4, None)?;
                self.unparen(value);
                self.push(&mut c, value);
                self.group(K::MathRoot, c, start)
            }
            K::Hash => {
                let start = self.pos;
                let mut c = Chain::EMPTY;
                self.take(Mode::Math, &mut c)?;
                let value = self.expr(0, Context::EMBED)?;
                self.push(&mut c, value);
                self.group(K::Math, c, start)
            }
            K::End | K::Dollar => self.missing(),
            K::Raw => self.raw(Mode::Math),
            K::RightParen
            | K::RightBracket
            | K::RightBrace
            | K::Comma
            | K::Semicolon
            | K::Colon
            | K::Bang => self.bump_as(Mode::Math, K::MathText),
            _ => self.bump(Mode::Math),
        }
    }
    fn math_args(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut c = Chain::EMPTY;
        self.take(Mode::Math, &mut c)?;
        while !matches!(self.kind(Mode::Math), K::End | K::RightParen | K::Dollar) {
            self.tick()?;
            self.spaces(Mode::Math, &mut c)?;
            if matches!(self.kind(Mode::Math), K::End | K::RightParen | K::Dollar) {
                break;
            }
            let before = self.pos;
            let mut arg = Chain::EMPTY;
            let first = self.peek(Mode::Math).unwrap();
            let after = self.token_at(Mode::Math, first.span.end as usize);
            let named = matches!(first.kind, K::MathIdent | K::MathText)
                && after.map(|t| t.kind) == Some(K::Colon);
            if named {
                let id = self.bump_as(Mode::Math, K::Ident)?;
                self.push(&mut arg, id);
                self.take(Mode::Math, &mut arg)?;
                self.spaces(Mode::Math, &mut arg)?;
            }
            let mut value = Chain::EMPTY;
            while !matches!(
                self.kind(Mode::Math),
                K::End | K::RightParen | K::Comma | K::Semicolon | K::Dollar
            ) {
                let (next, _) = self.ahead(Mode::Math);
                if trivia(self.kind(Mode::Math))
                    && next.map_or(true, |t| {
                        matches!(t.kind, K::RightParen | K::Comma | K::Semicolon | K::Dollar)
                    })
                {
                    break;
                }
                if self.kind(Mode::Math) == K::Hash {
                    self.take(Mode::Math, &mut value)?;
                    let id = self.expr(0, Context::EMBED)?;
                    self.push(&mut value, id);
                } else {
                    let id = if trivia(self.kind(Mode::Math)) {
                        self.bump(Mode::Math)?
                    } else {
                        self.math_expr(0, None)?
                    };
                    self.push(&mut value, id);
                }
            }
            let value_id = if value.first == NIL {
                self.missing()?
            } else if value.first == value.last {
                value.first
            } else {
                self.group(K::Math, value, before)?
            };
            let id = if named {
                self.push(&mut arg, value_id);
                self.group(K::Named, arg, before)?
            } else {
                value_id
            };
            self.push(&mut c, id);
            self.spaces(Mode::Math, &mut c)?;
            if matches!(self.kind(Mode::Math), K::Comma | K::Semicolon) {
                self.take(Mode::Math, &mut c)?;
            }
            if self.pos == before {
                let id = self.invalid(Mode::Math)?;
                self.push(&mut c, id);
            }
        }
        self.expect(Mode::Math, K::RightParen, &mut c)?;
        self.group(K::MathArgs, c, start)
    }
    fn raw(&mut self, mode: Mode) -> Result<u32, ParseError> {
        let token = self
            .peek(mode)
            .ok_or_else(|| self.failure(ErrorKind::UnexpectedToken))?;
        let start = token.span.start as usize;
        let end = token.span.end as usize;
        self.pos = end;
        self.cached = None;
        if let Some(error) = token.error {
            self.report(error, token.span)?;
        }
        let bytes = self.source.as_bytes();
        let run = bytes[start..end].iter().take_while(|&&b| b == b'`').count();
        let width = if run == 2 { 1 } else { run.max(1) };
        let closed = end >= start + 2 * width && bytes[end - width..end].iter().all(|&b| b == b'`');
        let close = if closed { end - width } else { end };
        let mut c = Chain::EMPTY;
        self.raw_leaf(K::RawDelim, start, (start + width).min(end), &mut c)?;
        let mut pos = (start + width).min(close);
        if width >= 3 {
            let lang_start = pos;
            while pos < close && !bytes[pos].is_ascii_whitespace() && bytes[pos] != b'`' {
                pos += 1;
            }
            if pos != lang_start {
                self.raw_leaf(K::RawLang, lang_start, pos, &mut c)?;
            }
            if pos < close && bytes[pos].is_ascii_whitespace() {
                let trim_start = pos;
                if bytes[pos] == b'\r' && bytes.get(pos + 1) == Some(&b'\n') {
                    pos += 2;
                } else {
                    pos += 1;
                }
                self.raw_leaf(K::RawTrimmed, trim_start, pos, &mut c)?;
            }
        }
        let mut text_start = pos;
        while pos < close {
            if matches!(bytes[pos], b'\n' | b'\r') {
                if text_start < pos {
                    self.raw_leaf(K::Text, text_start, pos, &mut c)?;
                }
                let newline_start = pos;
                if bytes[pos] == b'\r' && bytes.get(pos + 1) == Some(&b'\n') {
                    pos += 2;
                } else {
                    pos += 1;
                }
                self.raw_leaf(K::RawTrimmed, newline_start, pos, &mut c)?;
                text_start = pos;
            } else {
                pos += 1;
            }
        }
        if text_start < close {
            self.raw_leaf(K::Text, text_start, close, &mut c)?;
        }
        if closed {
            self.raw_leaf(K::RawDelim, close, end, &mut c)?;
        } else {
            let missing = self.node(
                K::Error,
                Span {
                    start: end as u32,
                    end: end as u32,
                },
                NIL,
            )?;
            self.push(&mut c, missing);
        }
        self.group(K::Raw, c, start)
    }
    fn raw_leaf(
        &mut self,
        kind: K,
        start: usize,
        end: usize,
        c: &mut Chain,
    ) -> Result<(), ParseError> {
        let id = self.node(
            kind,
            Span {
                start: start as u32,
                end: end as u32,
            },
            NIL,
        )?;
        self.push(c, id);
        Ok(())
    }
}

fn trivia(kind: K) -> bool {
    matches!(
        kind,
        K::Space | K::Parbreak | K::LineComment | K::BlockComment | K::Shebang
    )
}
fn closing(kind: K) -> bool {
    matches!(kind, K::RightParen | K::RightBracket | K::RightBrace)
}
fn is_newline(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}
fn binding(kind: K) -> Option<(u8, u8)> {
    Some(match kind {
        K::Eq | K::PlusEq | K::HyphEq | K::StarEq | K::SlashEq => (1, 1),
        K::Or => (2, 3),
        K::And => (3, 4),
        K::EqEq | K::ExclEq => (4, 5),
        K::Lt | K::LtEq | K::Gt | K::GtEq | K::In | K::Not => (5, 6),
        K::Plus | K::Minus => (6, 7),
        K::Star | K::Slash => (7, 8),
        _ => return None,
    })
}
