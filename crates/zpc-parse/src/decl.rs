//! Declaration parsing.
//!
//! Ported from `parse()`, `getclassspec()`, `declfuncvar()`, `declglb()`,
//! `declloc()`, `decl_const()`, `decl_enum()`, `funcstub()`, `newfunc()`,
//! `declargs()`, `doarg()`, `initials()` and `initvector()` in
//! `compiler/libpc300/sc1.c`.
//!
//! # Pawn quirks this file exists to get right
//!
//! * **A declaration is a function or a variable, and you cannot tell until the
//!   `(`.** `declfuncvar()` tries `newfunc()` first and falls back to `declglb()`;
//!   here the decision is made with one token of lookahead past the name.
//! * **Each declarator carries its own tag.** `new Float:a, b` leaves `b`
//!   untagged, because `declglb()` re-reads a tag at the top of every loop pass.
//! * **`{1, 3, ...}` extrapolates, it does not zero-fill.** The AST records only
//!   the trailing `...` ([`InitList::fill_rest`]); the arithmetic is a semantic
//!   concern (`initvector()` adds the difference of the last two values).
//! * **A leading `@` makes a symbol implicitly `public`** with no keyword.
//! * **`= sizeof other` / `= tagof other` parameter defaults are not
//!   expressions.** They name another *parameter* and are resolved only once the
//!   whole list has been read, which is why they get their own AST variants.

use zpc_ast::{
    Dim, Ident, Span, StateRef, TagRef, TagSpec,
    decl::{
        ConstDecl, Declarator, EnumDecl, EnumMember, EnumStep, EnumStepOp, FuncDecl, FuncKind,
        FuncModifiers, FuncName, Init, InitList, Item, NativeAlias, OverloadableOp, Param,
        ParamDecl, ParamDefault, Pragma, PragmaKind, RestParam, StateSpec, VarDecl, VarModifiers,
    },
};
use zpc_lex::TokenKind;

use crate::{MAX_DIMENSIONS, Parser};

impl Parser<'_> {
    /// One top-level declaration, mirroring the `switch` in `parse()`.
    ///
    /// Returns `None` when the construct produces no AST node (a preprocessor
    /// directive the preprocessor already handled, a stray `;`) or when recovery
    /// swallowed it. Every path consumes at least one token.
    pub(crate) fn parse_item(&mut self) -> Option<Item> {
        match self.peek() {
            TokenKind::Eof => None,
            // A stray terminator between declarations is harmless.
            TokenKind::Semi => {
                self.bump();
                None
            }
            TokenKind::PpPragma => self.parse_pragma().map(Item::Pragma),
            // Any other directive has already been dealt with by the
            // preprocessor; the scanner still emits it, so drop its whole line.
            k if is_directive(k) => {
                self.bump();
                self.skip_directive_line();
                None
            }
            TokenKind::New => {
                let start = self.cur_span();
                self.bump();
                let mods = self.parse_class_spec(VarModifiers::default());
                Some(self.parse_var_decl(mods, start, None, None))
            }
            TokenKind::Static | TokenKind::Public | TokenKind::Stock => {
                let start = self.cur_span();
                let initial = match self.peek() {
                    TokenKind::Static => VarModifiers { static_: true, ..Default::default() },
                    TokenKind::Public => VarModifiers { public: true, ..Default::default() },
                    _ => VarModifiers { stock: true, ..Default::default() },
                };
                self.bump();
                let mods = self.parse_class_spec(initial);
                self.parse_func_or_var(mods, start)
            }
            TokenKind::Const => Some(Item::Const(self.parse_const_decl())),
            TokenKind::Enum => Some(Item::Enum(self.parse_enum_decl())),
            TokenKind::Native => Some(self.parse_func_stub(FuncKind::Native)),
            TokenKind::Forward => Some(self.parse_func_stub(FuncKind::Forward)),
            // A bare name, `Tag:name` or `operator` at file scope: only a
            // function may start this way (`newfunc()` from `parse()`).
            TokenKind::Ident(_) | TokenKind::Label(_) | TokenKind::Operator => {
                let start = self.cur_span();
                self.parse_func_or_var(VarModifiers::default(), start)
            }
            TokenKind::RBrace => {
                let span = self.cur_span();
                self.bump();
                self.error(54, span, &[]); // unmatched closing brace
                Some(Item::Error(span))
            }
            TokenKind::LBrace => {
                let span = self.cur_span();
                self.error(55, span, &[]); // function body without a header
                let span = self.skip_braced();
                Some(Item::Error(span))
            }
            _ => {
                let span = self.cur_span();
                self.error(10, span, &[]); // invalid function or declaration
                self.resync_decl();
                Some(Item::Error(span.to(self.prev_span())))
            }
        }
    }

    // ------------------------------------------------------------ class specs

    /// `getclassspec()`: read any further `const`/`stock`/`static`/`public`
    /// keywords after the one that opened the declaration.
    ///
    /// Repeating a specifier, or combining `static` with `public`, is error 42.
    fn parse_class_spec(&mut self, initial: VarModifiers) -> VarModifiers {
        let mut mods = initial;
        let mut err: Option<Span> = None;
        loop {
            let span = self.cur_span();
            let flag = match self.peek() {
                TokenKind::Const => &mut mods.is_const,
                TokenKind::Stock => &mut mods.stock,
                TokenKind::Static => &mut mods.static_,
                TokenKind::Public => &mut mods.public,
                _ => break,
            };
            if *flag && err.is_none() {
                err = Some(span);
            }
            *flag = true;
            self.bump();
            if err.is_some() {
                break;
            }
        }
        if mods.static_ && mods.public {
            err = err.or(Some(self.prev_span()));
            mods.static_ = false;
            mods.public = false;
        }
        if let Some(span) = err {
            self.error(42, span, &[]); // invalid combination of class specifiers
        }
        mods
    }

    // ------------------------------------------------------- function or var

    /// `declfuncvar()`: decide between a function and a global variable.
    ///
    /// The original tries `newfunc()` and backs out to `declglb()` if no `(`
    /// follows the name. Here the same decision is made by looking one token past
    /// the name, which avoids the backtracking. `const`, and `public` combined
    /// with `stock`, are legal only on variables and force the variable reading.
    fn parse_func_or_var(&mut self, mods: VarModifiers, start: Span) -> Option<Item> {
        let tag = self.eat_tag();

        if self.at(&TokenKind::Operator) {
            return Some(Item::Func(self.parse_func(mods, start, tag)));
        }
        if self.at(&TokenKind::Native) {
            let span = self.cur_span();
            self.error(42, span, &[]); // native may not carry class specifiers
            self.resync_decl();
            return Some(Item::Error(start.to(self.prev_span())));
        }
        if !matches!(self.peek(), TokenKind::Ident(_)) {
            let (span, found) = (self.cur_span(), self.peek().describe());
            self.error(20, span, &[found]); // invalid symbol name
            self.resync_decl();
            return Some(Item::Error(start.to(self.prev_span())));
        }

        let must_be_var = mods.is_const || (mods.public && mods.stock);
        if !must_be_var && matches!(self.peek_at(1), TokenKind::LParen) {
            return Some(Item::Func(self.parse_func(mods, start, tag)));
        }
        let name = self.eat_ident();
        Some(self.parse_var_decl(mods, start, tag, name))
    }

    // --------------------------------------------------------------- variables

    /// `declglb()`/`declloc()`: `new Float:a, b[3] = {1, 2, 3};`
    ///
    /// `first_tag`/`first_name` carry the tag and name already consumed while
    /// deciding this was not a function. Every subsequent declarator reads its own
    /// tag, so `new Float:a, b` leaves `b` untagged.
    ///
    /// Also used by the statement parser for local declarations, which have the
    /// identical shape.
    pub(crate) fn parse_var_decl(
        &mut self,
        mods: VarModifiers,
        start: Span,
        first_tag: Option<TagRef>,
        first_name: Option<Ident>,
    ) -> Item {
        Item::Var(self.parse_var_decl_inner(mods, start, first_tag, first_name))
    }

    fn parse_var_decl_inner(
        &mut self,
        mut mods: VarModifiers,
        start: Span,
        first_tag: Option<TagRef>,
        first_name: Option<Ident>,
    ) -> VarDecl {
        let mut declarators = Vec::new();
        let mut tag = first_tag;
        let mut name = first_name;
        loop {
            let tag = tag.take().or_else(|| self.eat_tag());
            let dstart = tag.as_ref().map_or_else(|| self.cur_span(), |t| t.span);
            let Some(name) = name.take().or_else(|| self.expect_ident()) else {
                self.resync_decl();
                break;
            };
            // A `@`-prefixed global is public without the keyword.
            if name.is_implicitly_public() {
                mods.public = true;
            }
            let dims = self.parse_dims();
            let init = if self.eat(&TokenKind::Assign) { Some(self.parse_init()) } else { None };
            let span = dstart.to(self.prev_span());
            declarators.push(Declarator { name, tag, dims, init, span });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.eat_terminator();
        VarDecl { modifiers: mods, declarators, span: start.to(self.prev_span()) }
    }

    /// Zero or more `[...]` groups.
    ///
    /// An empty `[]` is an indeterminate dimension: legal for a parameter and for
    /// the last dimension of an initialised variable, where `initials()` deduces
    /// the size. A present size must fold to a constant, and its *tag* also
    /// becomes the dimension's index tag - `new data[Player]` is how an array gets
    /// an enum-typed index.
    fn parse_dims(&mut self) -> Vec<Dim> {
        let mut dims = Vec::new();
        while self.at(&TokenKind::LBracket) {
            let start = self.cur_span();
            self.bump();
            let size = if self.at(&TokenKind::RBracket) { None } else { Some(self.parse_expr()) };
            self.expect(&TokenKind::RBracket);
            if dims.len() == MAX_DIMENSIONS {
                self.error(53, start, &[]); // exceeding maximum number of dimensions
                continue;
            }
            dims.push(Dim { size, span: start.to(self.prev_span()) });
        }
        dims
    }

    /// `initials()`: the right-hand side of `=` in a declaration.
    fn parse_init(&mut self) -> Init {
        if self.at(&TokenKind::LBrace) { Init::List(self.parse_init_list()) } else { Init::Expr(self.parse_expr()) }
    }

    /// `initvector()`: `{ a, b, ... }`, nesting one level per dimension.
    ///
    /// A trailing `...` sets `fill_rest`. Pawn *extrapolates* from it - `{1, 3,
    /// ...}` continues 5, 7, 9 with the step taken from the last two values, and a
    /// single value such as `{7, ...}` repeats. Only an initialiser with no
    /// ellipsis at all is zero-filled. None of that arithmetic happens here; the
    /// flag is all the AST records.
    fn parse_init_list(&mut self) -> InitList {
        let start = self.cur_span();
        self.expect(&TokenKind::LBrace);
        let mut elems = Vec::new();
        let mut fill_rest = false;
        loop {
            if self.at(&TokenKind::RBrace) || self.at_eof() {
                break;
            }
            if self.eat(&TokenKind::Ellipsis) {
                fill_rest = true;
                break;
            }
            elems.push(self.parse_init());
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace);
        InitList { elems, fill_rest, span: start.to(self.prev_span()) }
    }

    // ---------------------------------------------------------------- const

    /// `decl_const()`: `const [Tag:]NAME = <constant expression>;`
    ///
    /// This creates a compile-time symbol with no storage, unlike a `const`
    /// *variable* (`new const x = 1`), which is a real cell that may not be
    /// assigned to.
    pub(crate) fn parse_const_decl(&mut self) -> ConstDecl {
        let start = self.cur_span();
        self.expect(&TokenKind::Const);
        let tag = self.eat_tag();
        let name = self.expect_ident().unwrap_or_else(|| Ident::new("", self.cur_span()));
        self.expect(&TokenKind::Assign);
        let value = self.parse_expr();
        self.eat_terminator();
        ConstDecl { tag, name, value, span: start.to(self.prev_span()) }
    }

    // ----------------------------------------------------------------- enum

    /// `decl_enum()`: `enum [Tag:] [Name] [(step)] { members }`.
    ///
    /// The explicit `Tag:` and the name are separate: `enum _: { .. }` forces the
    /// members untagged, while `enum Colour { .. }` makes the *name* the tag. The
    /// step operators are the compound-assignment tokens `+=`, `*=` and `<<=`, and
    /// only those three (`decl_enum()` silently ignores anything else inside the
    /// parentheses).
    pub(crate) fn parse_enum_decl(&mut self) -> EnumDecl {
        let start = self.cur_span();
        self.expect(&TokenKind::Enum);
        let tag = self.eat_tag();
        let name = self.eat_ident();

        let step = if self.at(&TokenKind::LParen) {
            let sstart = self.cur_span();
            self.bump();
            let op = match self.peek() {
                TokenKind::PlusAssign => Some(EnumStepOp::Add),
                TokenKind::StarAssign => Some(EnumStepOp::Mult),
                TokenKind::ShlAssign => Some(EnumStepOp::Shl),
                _ => None,
            };
            let step = match op {
                Some(op) => {
                    self.bump();
                    let value = self.parse_expr();
                    Some(EnumStep { op, value, span: sstart.to(self.prev_span()) })
                }
                None => {
                    let (span, found) = (self.cur_span(), self.peek().describe());
                    self.error(1, span, &["+=", found]);
                    None
                }
            };
            self.expect(&TokenKind::RParen);
            step
        } else {
            None
        };

        let mut members = Vec::new();
        if self.expect(&TokenKind::LBrace) {
            loop {
                // `matchtoken('}')` first, so a trailing comma is accepted.
                if self.at(&TokenKind::RBrace) || self.at_eof() {
                    break;
                }
                let Some(member) = self.parse_enum_member() else { break };
                members.push(member);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace);
        } else {
            self.resync_decl();
        }
        // `matchtoken(';')` - the terminator is optional after the closing brace.
        self.eat(&TokenKind::Semi);
        EnumDecl { tag, name, step, members, span: start.to(self.prev_span()) }
    }

    /// One enum member: `[Tag:]NAME [ [size] ] [= value]`.
    ///
    /// `[size]` does not declare an array; it makes the member span several cells
    /// so `arr[Coords]` addresses a sub-range, and it advances the next member's
    /// value by that amount. That is how Pawn 3 fakes a struct.
    fn parse_enum_member(&mut self) -> Option<EnumMember> {
        let tag = self.eat_tag();
        let start = tag.as_ref().map_or_else(|| self.cur_span(), |t| t.span);
        let name = self.expect_ident()?;
        let size = if self.eat(&TokenKind::LBracket) {
            let e = if self.at(&TokenKind::RBracket) { None } else { Some(self.parse_expr()) };
            self.expect(&TokenKind::RBracket);
            e
        } else {
            None
        };
        let value = if self.eat(&TokenKind::Assign) { Some(self.parse_expr()) } else { None };
        Some(EnumMember { tag, name, size, value, span: start.to(self.prev_span()) })
    }

    // ------------------------------------------------------------- functions

    /// `funcstub()`: a `native` or `forward` declaration.
    ///
    /// Neither has a body. `native` additionally accepts an alias (`= bar`) or a
    /// fixed, conventionally negative, native index (`= -3`); the alias is
    /// mandatory for a native operator overload, which has no exportable name of
    /// its own. A `native` may not carry class specifiers (error 42), whereas a
    /// `forward` silently accepts and ignores them.
    fn parse_func_stub(&mut self, kind: FuncKind) -> Item {
        let start = self.cur_span();
        self.bump(); // `native` / `forward`
        let return_tag = self.eat_tag();
        let return_dims = self.parse_return_dims();

        let mut mods = FuncModifiers::default();
        loop {
            let (flag, span) = match self.peek() {
                TokenKind::Public => (&mut mods.public, self.cur_span()),
                TokenKind::Stock => (&mut mods.stock, self.cur_span()),
                TokenKind::Static => (&mut mods.static_, self.cur_span()),
                _ => break,
            };
            *flag = true;
            self.bump();
            if kind == FuncKind::Native {
                self.error(42, span, &[]);
            }
        }

        let Some(name) = self.parse_func_name() else {
            self.resync_decl();
            return Item::Error(start.to(self.prev_span()));
        };
        let is_operator = matches!(name, FuncName::Operator { .. });
        if let FuncName::Ident(id) = &name
            && id.is_implicitly_public()
        {
            if kind == FuncKind::Native {
                let span = id.span;
                self.error(42, span, &[]);
            } else {
                mods.public = true;
            }
        }

        self.expect(&TokenKind::LParen);
        let params = self.parse_params();
        let states = self.parse_states();
        if let Some(s) = &states {
            if kind == FuncKind::Native || is_operator {
                self.error(82, s.span, &[]); // natives/operators may not have states
            } else {
                self.error(231, s.span, &[]); // state spec on a forward is ignored
            }
        }

        let alias = if kind == FuncKind::Native { self.parse_native_alias(is_operator) } else { None };
        self.eat_terminator();

        Item::Func(FuncDecl {
            kind,
            modifiers: mods,
            return_tag,
            return_dims,
            name,
            params,
            states,
            alias,
            body: None,
            span: start.to(self.prev_span()),
        })
    }

    /// The `[3]` groups of a function returning an array: `Float:make_vec()[3]`.
    ///
    /// Unlike an argument's dimensions these may not be indeterminate - a
    /// zero-sized return array is error 9.
    fn parse_return_dims(&mut self) -> Vec<Dim> {
        let dims = self.parse_dims();
        for d in &dims {
            if d.size.is_none() {
                self.error(9, d.span, &[]); // invalid array size
            }
        }
        dims
    }

    /// `= bar` or `= -3` after a native's parameter list.
    fn parse_native_alias(&mut self, is_operator: bool) -> Option<NativeAlias> {
        if is_operator && !self.at(&TokenKind::Assign) {
            // A native operator must name the host function it binds to.
            self.expect(&TokenKind::Assign);
            return None;
        }
        if !self.eat(&TokenKind::Assign) {
            return None;
        }
        match self.eat_ident() {
            Some(id) => Some(NativeAlias::Symbol(id)),
            None => Some(NativeAlias::Index(self.parse_expr())),
        }
    }

    /// `newfunc()`: an ordinary function definition or an old-style prototype.
    ///
    /// Note that `newfunc()` has no return-dimension loop: only a `native` or a
    /// `forward` may *declare* an array return (`native Float:[3] make_vec();`).
    /// For an ordinary function the shape is inferred from its `return`
    /// statement, so `return_dims` stays empty here.
    fn parse_func(&mut self, mods: VarModifiers, start: Span, return_tag: Option<TagRef>) -> FuncDecl {
        let mut modifiers =
            FuncModifiers { public: mods.public, stock: mods.stock, static_: mods.static_ };
        let name = self
            .parse_func_name()
            .unwrap_or_else(|| FuncName::Ident(Ident::new("", self.cur_span())));
        let is_operator = matches!(name, FuncName::Operator { .. });
        // `@name()` is public without the keyword; combining it with `stock` is
        // error 42.
        if let FuncName::Ident(id) = &name
            && id.is_implicitly_public()
        {
            modifiers.public = true;
            if modifiers.stock {
                let span = id.span;
                self.error(42, span, &[]);
            }
        }

        self.expect(&TokenKind::LParen);
        let params = self.parse_params();
        let states = self.parse_states();
        if is_operator && let Some(s) = &states {
            self.error(82, s.span, &[]); // operators may not have states
        }

        // `newfunc()` treats an EXPLICIT `;` here as an old-style prototype:
        // `if (matchtoken(';'))`. A line break does NOT - it warns 218 ("old style
        // prototypes used with optional semicolumns") only when the semicolon is
        // really there. Treating a newline as a prototype terminator broke every
        // braceless function body, which is how `float.inc` and everything that
        // includes it failed with error 010.
        let body = if self.at(&TokenKind::LBrace) {
            Some(self.parse_block())
        } else if self.eat(&TokenKind::Semi) {
            None
        } else {
            // Pawn permits a single statement as a whole function body.
            let stmt = self.parse_stmt();
            let span = self.prev_span();
            Some(zpc_ast::stmt::Block { stmts: vec![stmt], span })
        };

        FuncDecl {
            kind: FuncKind::Normal,
            modifiers,
            return_tag,
            return_dims: Vec::new(),
            name,
            params,
            states,
            alias: None,
            body,
            span: start.to(self.prev_span()),
        }
    }

    /// A function's name: an identifier or `operator<op>`.
    fn parse_func_name(&mut self) -> Option<FuncName> {
        if self.at(&TokenKind::Operator) {
            let start = self.cur_span();
            self.bump();
            let op = self.parse_operator_name()?;
            return Some(FuncName::Operator { op, span: start.to(self.prev_span()) });
        }
        self.expect_ident().map(FuncName::Ident)
    }

    /// `operatorname()`: the complete set of overloadable operators.
    ///
    /// Anything else is error 7. Note the absentees: `[]`, `()`, `&&`, `||` and
    /// the compound assignments are not overloadable, and `~` is the
    /// destructor-like operator whose single argument must be an array.
    fn parse_operator_name(&mut self) -> Option<OverloadableOp> {
        let op = match self.peek() {
            TokenKind::Plus => OverloadableOp::Add,
            TokenKind::Minus => OverloadableOp::Sub,
            TokenKind::Star => OverloadableOp::Mul,
            TokenKind::Slash => OverloadableOp::Div,
            TokenKind::Percent => OverloadableOp::Mod,
            TokenKind::Gt => OverloadableOp::Gt,
            TokenKind::Lt => OverloadableOp::Lt,
            TokenKind::Not => OverloadableOp::Not,
            TokenKind::Tilde => OverloadableOp::BitNot,
            TokenKind::Assign => OverloadableOp::Assign,
            TokenKind::PlusPlus => OverloadableOp::Inc,
            TokenKind::MinusMinus => OverloadableOp::Dec,
            TokenKind::EqEq => OverloadableOp::Eq,
            TokenKind::NotEq => OverloadableOp::Ne,
            TokenKind::LtEq => OverloadableOp::Le,
            TokenKind::GtEq => OverloadableOp::Ge,
            _ => {
                let span = self.cur_span();
                self.error(7, span, &[]); // operator cannot be redefined
                return None;
            }
        };
        self.bump();
        Some(op)
    }

    // ------------------------------------------------------------- arguments

    /// `declargs()`/`doarg()`: the parameter list, `(` already consumed.
    ///
    /// A parameter is a run of prefix markers (`const`, `&`, one or several tags)
    /// followed by a name, optional dimensions and an optional default. `...` ends
    /// the list; anything after it is unreachable, so the loop stops there exactly
    /// as `declargs()`'s `tok!=tELLIPS && matchtoken(',')` condition does.
    fn parse_params(&mut self) -> Vec<Param> {
        let mut params = Vec::new();
        if self.eat(&TokenKind::RParen) {
            return params;
        }
        loop {
            let start = self.cur_span();
            let mut by_ref = false;
            let mut is_const = false;
            let mut tags: Option<TagSpec> = None;

            // Prefix markers, in any order (`declargs()` loops over them).
            loop {
                match self.peek() {
                    TokenKind::Amp => {
                        by_ref = true;
                        self.bump();
                    }
                    TokenKind::Const => {
                        is_const = true;
                        self.bump();
                    }
                    TokenKind::Label(_) => {
                        let t = self.eat_tag().expect("peeked a label");
                        let span = t.span;
                        tags = Some(TagSpec { tags: vec![t], span });
                    }
                    // `{Float,_}:` - several alternative tags, which only a
                    // parameter may have.
                    TokenKind::LBrace => tags = Some(self.parse_multi_tag()),
                    _ => break,
                }
            }

            if self.at(&TokenKind::Ellipsis) {
                let span = self.cur_span();
                self.bump();
                params.push(Param::Rest(RestParam { tags, span: start.to(span) }));
                break; // `...` must be last
            }

            let Some(name) = self.expect_ident() else {
                self.skip_to_param_end();
                break;
            };
            if name.is_implicitly_public() {
                let span = name.span;
                let text = name.name.clone();
                self.error(56, span, &[&text]); // arguments cannot be public
            }

            let dims = self.parse_dims();
            if by_ref && !dims.is_empty() {
                let (span, text) = (name.span, name.name.clone());
                self.error(67, span, &[&text]); // both a reference and an array
            }
            let default = if self.eat(&TokenKind::Assign) {
                Some(self.parse_param_default(&dims))
            } else {
                None
            };

            params.push(Param::Fixed(ParamDecl {
                name,
                tags,
                by_ref,
                is_const,
                dims,
                default,
                span: start.to(self.prev_span()),
            }));

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen);
        params
    }

    /// `{Float,_}:` - up to `MAXTAGS` (16) alternative tags for one parameter.
    fn parse_multi_tag(&mut self) -> TagSpec {
        let start = self.cur_span();
        self.expect(&TokenKind::LBrace);
        let mut tags = Vec::new();
        while let Some(id) = self.expect_ident() {
            let span = id.span;
            tags.push(TagRef { name: id, span });
            if self.eat(&TokenKind::RBrace) {
                // `needtoken(':')` closes the group.
                self.expect(&TokenKind::Colon);
                return TagSpec { tags, span: start.to(self.prev_span()) };
            }
            if !self.expect(&TokenKind::Comma) {
                break;
            }
        }
        self.eat(&TokenKind::RBrace);
        self.eat(&TokenKind::Colon);
        TagSpec { tags, span: start.to(self.prev_span()) }
    }

    /// The `= ...` of a parameter (`doarg()`).
    ///
    /// The shape depends on whether the parameter is an array:
    ///
    /// * array: `= symbol` uses the address of an existing *global* array
    ///   directly, anything else is a normal initialiser copied per call (unless
    ///   the parameter is `const`);
    /// * scalar or reference: `= sizeof other` / `= tagof other` name another
    ///   *parameter* and are resolved after the whole list is read - that is how
    ///   `stock copy(dest[], len = sizeof dest)` works - and anything else is a
    ///   constant expression.
    fn parse_param_default(&mut self, dims: &[Dim]) -> ParamDefault {
        if !dims.is_empty() {
            return match self.eat_ident() {
                Some(id) => ParamDefault::Symbol(id),
                None => ParamDefault::Array(self.parse_init()),
            };
        }
        let start = self.cur_span();
        let is_sizeof = self.at(&TokenKind::Sizeof);
        if is_sizeof || self.at(&TokenKind::Tagof) {
            self.bump();
            // `while (matchtoken('('))` - the parentheses are optional and may
            // even be repeated.
            let mut parens = 0usize;
            while self.eat(&TokenKind::LParen) {
                parens += 1;
            }
            let arg = self.expect_ident();
            let mut levels: u8 = 0;
            if is_sizeof {
                while self.eat(&TokenKind::LBracket) {
                    levels = levels.saturating_add(1);
                    self.expect(&TokenKind::RBracket);
                }
            }
            for _ in 0..parens {
                self.expect(&TokenKind::RParen);
            }
            let span = start.to(self.prev_span());
            let arg = arg.unwrap_or_else(|| Ident::new("", span));
            return if is_sizeof {
                ParamDefault::SizeOf { arg, levels, span }
            } else {
                ParamDefault::TagOf { arg, span }
            };
        }
        ParamDefault::Expr(self.parse_expr())
    }

    /// Recovery inside a parameter list: drop tokens up to the next `,` or the
    /// closing `)`, so one bad parameter does not poison the rest of the header.
    fn skip_to_param_end(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.peek() {
                TokenKind::Eof => return,
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen if depth == 0 => return,
                TokenKind::Comma if depth == 0 => return,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    depth = depth.saturating_sub(1)
                }
                _ => {}
            }
            self.bump();
        }
    }

    // ---------------------------------------------------------------- states

    /// `getstates()`: `<idle, running>`, `<engine:idle>` or the fall-back `<>`.
    ///
    /// Pawn lets one function name have several implementations, one per state
    /// combination, and dispatches on the automaton's current state at run time.
    fn parse_states(&mut self) -> Option<StateSpec> {
        if !self.at(&TokenKind::Lt) {
            return None;
        }
        let start = self.cur_span();
        self.bump();
        if self.eat(&TokenKind::Gt) {
            return Some(StateSpec {
                states: Vec::new(),
                fallback: true,
                span: start.to(self.prev_span()),
            });
        }
        let mut states = Vec::new();
        loop {
            let sstart = self.cur_span();
            // `automaton:state` - the automaton name lexes as a label.
            let automaton = self.eat_tag().map(|t| t.name);
            let Some(state) = self.expect_ident() else { break };
            states.push(StateRef { automaton, state, span: sstart.to(self.prev_span()) });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::Gt);
        Some(StateSpec { states, fallback: false, span: start.to(self.prev_span()) })
    }

    // --------------------------------------------------------------- pragmas

    /// A `#pragma` that survives preprocessing.
    ///
    /// The tail is kept verbatim: each pragma's argument syntax is different (an
    /// expression, a quoted string, a bare word, a comma-separated symbol list),
    /// and `sc2.c` scans each one ad hoc, so pre-parsing here would bake one
    /// reading into the shared AST.
    fn parse_pragma(&mut self) -> Option<Pragma> {
        let start = self.cur_span();
        self.bump(); // `#pragma`
        let Some(name) = self.eat_ident() else {
            self.error(207, start, &[]); // unknown #pragma
            self.skip_directive_line();
            return None;
        };
        let kind = pragma_kind(&name.name);
        if kind == PragmaKind::Unknown {
            self.error(207, name.span, &[]);
        }
        // Everything from just after the name to the end of the physical line.
        let mut end = name.span.end;
        while !self.at_eof() && !self.tok().line_start {
            end = self.cur_span().end;
            self.bump();
        }
        let args = self.text(Span::new(name.span.end, end)).trim().to_string();
        Some(Pragma { kind, name, args, span: start.to(Span::at(end)) })
    }

    /// Drop the remainder of a directive's physical line. The scanner emits the
    /// directive keyword and then tokenises the rest of the line normally, so
    /// `#include <amxmodx>` leaves `<`, `amxmodx`, `>` behind.
    fn skip_directive_line(&mut self) {
        while !self.at_eof() && !self.tok().line_start {
            self.bump();
        }
    }
}

/// True for the preprocessor directives the scanner surfaces. All of them are
/// consumed by the preprocessor; only `#pragma` produces an AST node.
fn is_directive(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::PpAssert
            | TokenKind::PpDefine
            | TokenKind::PpElse
            | TokenKind::PpElseIf
            | TokenKind::PpEmit
            | TokenKind::PpEndIf
            | TokenKind::PpEndInput
            | TokenKind::PpEndScript
            | TokenKind::PpError
            | TokenKind::PpFile
            | TokenKind::PpIf
            | TokenKind::PpInclude
            | TokenKind::PpLine
            | TokenKind::PpPragma
            | TokenKind::PpTryInclude
            | TokenKind::PpUndef
    )
}

/// The pragmas `sc2.c` recognises; anything else is warning 207.
fn pragma_kind(name: &str) -> PragmaKind {
    match name {
        "amxlimit" => PragmaKind::AmxLimit,
        "codepage" => PragmaKind::CodePage,
        "compress" => PragmaKind::Compress,
        "ctrlchar" => PragmaKind::CtrlChar,
        "deprecated" => PragmaKind::Deprecated,
        "dynamic" => PragmaKind::Dynamic,
        "library" => PragmaKind::Library,
        "reqlib" => PragmaKind::ReqLib,
        "reqclass" => PragmaKind::ReqClass,
        "loadlib" => PragmaKind::LoadLib,
        "explib" => PragmaKind::ExpLib,
        "expclass" => PragmaKind::ExpClass,
        "defclasslib" => PragmaKind::DefClassLib,
        "pack" => PragmaKind::Pack,
        "rational" => PragmaKind::Rational,
        "semicolon" => PragmaKind::Semicolon,
        "tabsize" => PragmaKind::TabSize,
        "align" => PragmaKind::Align,
        "unused" => PragmaKind::Unused,
        "showstackusageinfo" => PragmaKind::ShowStackUsageInfo,
        _ => PragmaKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zpc_ast::Program;
    use zpc_diag::Diagnostics;
    use zpc_lex::Scanner;

    fn parse_src(src: &str) -> (Program, Diagnostics) {
        let mut lexdiags = Diagnostics::new();
        let toks = Scanner::new(src, "test.sma").scan(&mut lexdiags);
        assert_eq!(lexdiags.error_count(), 0, "the fixture must lex cleanly");
        crate::parse(src, &toks, "test.sma")
    }

    fn parse_clean(src: &str) -> Program {
        let (program, diags) = parse_src(src);
        let shown: Vec<_> = diags.items().iter().map(|d| (d.code, d.message.clone())).collect();
        assert!(shown.is_empty(), "unexpected diagnostics: {shown:?}");
        program
    }

    fn var(program: &Program, idx: usize) -> &VarDecl {
        match &program.items[idx] {
            Item::Var(v) => v,
            other => panic!("expected a variable, got {other:?}"),
        }
    }

    fn func(program: &Program, idx: usize) -> &FuncDecl {
        match &program.items[idx] {
            Item::Func(f) => f,
            other => panic!("expected a function, got {other:?}"),
        }
    }

    // ------------------------------------------------------------ variables

    #[test]
    fn each_declarator_reads_its_own_tag() {
        let p = parse_clean("new Float:a, b, Float:c;");
        let v = var(&p, 0);
        assert_eq!(v.declarators.len(), 3);
        assert_eq!(v.declarators[0].tag.as_ref().unwrap().name.name, "Float");
        assert!(v.declarators[1].tag.is_none(), "`b` in `new Float:a, b` is untagged");
        assert_eq!(v.declarators[2].tag.as_ref().unwrap().name.name, "Float");
    }

    #[test]
    fn class_specifiers_combine() {
        let p = parse_clean("new const g_readonly = 100;\nstatic g_file;\nstock g_maybe;");
        assert!(var(&p, 0).modifiers.is_const);
        assert!(var(&p, 1).modifiers.static_);
        assert!(var(&p, 2).modifiers.stock);
    }

    #[test]
    fn static_and_public_together_is_error_42() {
        let (_, d) = parse_src("static public g_bad;");
        assert!(d.items().iter().any(|x| x.code == 42));
    }

    #[test]
    fn a_repeated_class_specifier_is_error_42() {
        let (_, d) = parse_src("new const const g_bad = 1;");
        assert!(d.items().iter().any(|x| x.code == 42));
    }

    #[test]
    fn an_at_prefixed_global_is_implicitly_public() {
        let p = parse_clean("new @g_shared;");
        assert!(var(&p, 0).modifiers.public);
        assert!(var(&p, 0).declarators[0].name.is_implicitly_public());
    }

    #[test]
    fn dimensions_may_be_indeterminate_when_initialised() {
        let p = parse_clean("new g_matrix[4][8];\nnew g_inferred[] = {1, 2, 3};");
        assert_eq!(var(&p, 0).declarators[0].dims.len(), 2);
        let inferred = &var(&p, 1).declarators[0];
        assert_eq!(inferred.dims.len(), 1);
        assert!(inferred.dims[0].size.is_none(), "`[]` is deduced from the initialiser");
    }

    #[test]
    fn too_many_dimensions_is_error_53() {
        let (_, d) = parse_src("new g[1][2][3][4][5];");
        assert!(d.items().iter().any(|x| x.code == 53));
    }

    #[test]
    fn the_ellipsis_marks_the_list_not_the_elements() {
        // `{1, 3, ...}` extrapolates 5, 7, 9 - the parser records only the flag.
        let p = parse_clean("new g[6] = {1, 3, ...};");
        let Some(Init::List(list)) = &var(&p, 0).declarators[0].init else {
            panic!("expected an initialiser list")
        };
        assert_eq!(list.elems.len(), 2, "the `...` is not an element");
        assert!(list.fill_rest);
    }

    #[test]
    fn nested_initialisers_nest_one_level_per_dimension() {
        let p = parse_clean("new g[2][3] = { {1, 2, 3}, {4, ...} };");
        let Some(Init::List(outer)) = &var(&p, 0).declarators[0].init else {
            panic!("expected a list")
        };
        assert!(!outer.fill_rest);
        assert_eq!(outer.elems.len(), 2);
        let Init::List(second) = &outer.elems[1] else { panic!("expected a nested list") };
        assert!(second.fill_rest);
    }

    #[test]
    fn a_string_initialiser_is_a_plain_expression() {
        let p = parse_clean("new g_string[] = \"text\";");
        assert!(matches!(var(&p, 0).declarators[0].init, Some(Init::Expr(_))));
    }

    #[test]
    fn semicolons_are_optional() {
        let p = parse_clean("new a\nnew b\n");
        assert_eq!(p.items.len(), 2);
    }

    // ----------------------------------------------------------------- enum

    #[test]
    fn an_anonymous_enum_has_neither_tag_nor_name() {
        let p = parse_clean("enum { A, B, C };");
        let Item::Enum(e) = &p.items[0] else { panic!("expected an enum") };
        assert!(e.tag.is_none() && e.name.is_none());
        assert_eq!(e.members.len(), 3);
    }

    #[test]
    fn a_named_enum_keeps_its_name_and_accepts_a_trailing_comma() {
        let p = parse_clean("enum CsArmorType\n{\n\tCS_ARMOR_NONE = 0,\n\tCS_ARMOR_KEVLAR = 1,\n};");
        let Item::Enum(e) = &p.items[0] else { panic!("expected an enum") };
        assert_eq!(e.name.as_ref().unwrap().name, "CsArmorType");
        assert_eq!(e.members.len(), 2);
        assert!(e.members[0].value.is_some());
    }

    #[test]
    fn an_explicit_tag_is_distinct_from_the_name() {
        let p = parse_clean("enum _: Anon { A };\nenum Bit: { B };");
        let Item::Enum(a) = &p.items[0] else { panic!("expected an enum") };
        assert!(a.tag.as_ref().unwrap().is_untagged(), "`_:` forces members untagged");
        assert_eq!(a.name.as_ref().unwrap().name, "Anon");
        let Item::Enum(b) = &p.items[1] else { panic!("expected an enum") };
        assert_eq!(b.tag.as_ref().unwrap().name.name, "Bit");
        assert!(b.name.is_none());
    }

    #[test]
    fn all_three_enum_steps_parse() {
        let p = parse_clean("enum A (+= 100) { X };\nenum B (*= 2) { Y };\nenum C (<<= 1) { Z };");
        let ops: Vec<_> = (0..3)
            .map(|i| {
                let Item::Enum(e) = &p.items[i] else { panic!("expected an enum") };
                e.step.as_ref().unwrap().op
            })
            .collect();
        assert_eq!(ops, vec![EnumStepOp::Add, EnumStepOp::Mult, EnumStepOp::Shl]);
    }

    #[test]
    fn enum_members_carry_sizes_tags_and_values() {
        let p = parse_clean("enum PlayerData { pd_name[32], pd_score, Float:pd_position[3] };");
        let Item::Enum(e) = &p.items[0] else { panic!("expected an enum") };
        assert!(e.members[0].size.is_some(), "pd_name[32] is a sized field");
        assert!(e.members[1].size.is_none() && e.members[1].value.is_none());
        assert_eq!(e.members[2].tag.as_ref().unwrap().name.name, "Float");
        assert!(e.members[2].size.is_some());
    }

    // ---------------------------------------------------------------- const

    #[test]
    fn a_const_declaration_may_be_tagged() {
        let p = parse_clean("const Float:MAX_SPEED = 2000.0;\nconst PLAIN = 1;");
        let Item::Const(c) = &p.items[0] else { panic!("expected a const") };
        assert_eq!(c.tag.as_ref().unwrap().name.name, "Float");
        assert_eq!(c.name.name, "MAX_SPEED");
        let Item::Const(c2) = &p.items[1] else { panic!("expected a const") };
        assert!(c2.tag.is_none());
    }

    // ------------------------------------------------------------- functions

    #[test]
    fn a_function_is_told_from_a_variable_by_the_paren() {
        let p = parse_clean("stock helper(a) { }\nstock g_var = 1;");
        assert!(matches!(p.items[0], Item::Func(_)));
        assert!(matches!(p.items[1], Item::Var(_)));
    }

    #[test]
    fn const_forces_the_variable_reading() {
        // `const` is legal on a variable only, so `declfuncvar()` never tries a
        // function - even though a `(` follows.
        let p = parse_clean("static const g_table[2] = {1, 2};");
        assert!(var(&p, 0).modifiers.is_const);
    }

    #[test]
    fn a_function_may_declare_an_array_return() {
        // `funcstub()` reads the dimensions between the return tag and the name.
        let p = parse_clean("native Float:[3] make_vec();");
        let f = func(&p, 0);
        assert_eq!(f.return_tag.as_ref().unwrap().name.name, "Float");
        assert_eq!(f.return_dims.len(), 1);
        assert!(f.return_dims[0].size.is_some());
    }

    #[test]
    fn an_indeterminate_return_dimension_is_error_9() {
        let (_, d) = parse_src("native Float:[] make_vec();");
        assert!(d.items().iter().any(|x| x.code == 9));
    }

    #[test]
    fn an_ordinary_function_declares_no_return_dimensions() {
        // `newfunc()` has no return-dimension loop; the shape comes from `return`.
        let p = parse_clean("Float:make_vec() { }");
        assert!(func(&p, 0).return_dims.is_empty());
    }

    #[test]
    fn native_and_forward_have_no_body() {
        let p = parse_clean("native my_native(id, const message[], len);\nforward my_forward(id, Float:value);");
        assert_eq!(func(&p, 0).kind, FuncKind::Native);
        assert!(func(&p, 0).body.is_none());
        assert_eq!(func(&p, 1).kind, FuncKind::Forward);
        assert!(func(&p, 1).body.is_none());
    }

    #[test]
    fn native_aliases_take_a_name_or_a_fixed_index() {
        let p = parse_clean("native foo(a) = bar;\nnative baz(a) = -3;");
        match func(&p, 0).alias.as_ref().unwrap() {
            NativeAlias::Symbol(id) => assert_eq!(id.name, "bar"),
            other => panic!("expected a symbol alias, got {other:?}"),
        }
        assert!(matches!(func(&p, 1).alias, Some(NativeAlias::Index(_))));
    }

    #[test]
    fn a_prototype_has_no_body() {
        let p = parse_clean("helper(a, b);\nhelper(a, b) { }");
        assert!(func(&p, 0).body.is_none(), "`helper(a, b);` is an old-style prototype");
        assert!(func(&p, 1).body.is_some());
    }

    #[test]
    fn an_at_prefixed_function_is_implicitly_public() {
        let p = parse_clean("@task_think(id) { }");
        assert!(func(&p, 0).modifiers.public);
    }

    #[test]
    fn operator_overloads_parse_with_a_return_tag() {
        let p = parse_clean("stock CsArmorType:operator+(CsArmorType:a, CsArmorType:b) { }");
        let f = func(&p, 0);
        assert!(matches!(f.name, FuncName::Operator { op: OverloadableOp::Add, .. }));
        assert_eq!(f.return_tag.as_ref().unwrap().name.name, "CsArmorType");
        assert_eq!(f.params.len(), 2);
    }

    #[test]
    fn every_overloadable_operator_is_accepted() {
        for op in ["+", "-", "*", "/", "%", ">", "<", "!", "~", "=", "++", "--", "==", "!=", "<=", ">="] {
            let src = format!("stock operator{op}(a, b) {{ }}");
            let (p, d) = parse_src(&src);
            assert_eq!(d.error_count(), 0, "operator{op} should parse");
            assert!(matches!(func(&p, 0).name, FuncName::Operator { .. }));
        }
    }

    #[test]
    fn a_bad_operator_name_is_error_7() {
        let (_, d) = parse_src("stock operator&&(a, b) { }");
        assert!(d.items().iter().any(|x| x.code == 7));
    }

    #[test]
    fn state_lists_and_the_fallback_parse() {
        let p = parse_clean("handler() <engine:idle, running> { }\nhandler() <> { }");
        let s = func(&p, 0).states.as_ref().unwrap();
        assert_eq!(s.states.len(), 2);
        assert_eq!(s.states[0].automaton.as_ref().unwrap().name, "engine");
        assert_eq!(s.states[0].state.name, "idle");
        assert!(s.states[1].automaton.is_none());
        assert!(func(&p, 1).states.as_ref().unwrap().fallback);
    }

    #[test]
    fn states_on_a_native_are_error_82_and_on_a_forward_warning_231() {
        let (_, d) = parse_src("native foo() <idle>;");
        assert!(d.items().iter().any(|x| x.code == 82));
        let (_, d) = parse_src("forward foo() <idle>;");
        assert!(d.items().iter().any(|x| x.code == 231));
    }

    // ------------------------------------------------------------ parameters

    #[test]
    fn parameters_carry_refs_consts_tags_and_arrays() {
        let p = parse_clean("stock f(id, Float:scale, &result, const label[], Float:vec[3]) { }");
        let ps = &func(&p, 0).params;
        assert_eq!(ps.len(), 5);
        let Param::Fixed(scale) = &ps[1] else { panic!("expected a fixed param") };
        assert_eq!(scale.tags.as_ref().unwrap().tags[0].name.name, "Float");
        let Param::Fixed(result) = &ps[2] else { panic!("expected a fixed param") };
        assert!(result.by_ref);
        let Param::Fixed(label) = &ps[3] else { panic!("expected a fixed param") };
        assert!(label.is_const);
        assert_eq!(label.dims.len(), 1);
        assert!(label.dims[0].size.is_none(), "`label[]` is indeterminate");
    }

    #[test]
    fn a_reference_to_an_array_is_error_67() {
        let (_, d) = parse_src("stock f(&bad[4]) { }");
        assert!(d.items().iter().any(|x| x.code == 67));
    }

    #[test]
    fn a_multi_tag_parameter_lists_its_alternatives() {
        let p = parse_clean("stock f({Float,_}:value) { }");
        let Param::Fixed(v) = &func(&p, 0).params[0] else { panic!("expected a fixed param") };
        let tags = &v.tags.as_ref().unwrap().tags;
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].name.name, "Float");
        assert!(tags[1].is_untagged());
    }

    #[test]
    fn the_rest_parameter_may_be_tagged() {
        let p = parse_clean("stock f(a, {Float,_}:...) { }\nstock g(a, ...) { }");
        let Param::Rest(r) = &func(&p, 0).params[1] else { panic!("expected a rest param") };
        assert_eq!(r.tags.as_ref().unwrap().tags.len(), 2);
        let Param::Rest(r2) = &func(&p, 1).params[1] else { panic!("expected a rest param") };
        assert!(r2.tags.is_none());
    }

    #[test]
    fn defaults_cover_expressions_arrays_and_symbols() {
        let p = parse_clean(
            "stock f(a = 5, const label[] = \"\", data[3] = {1, 2, 3}, table[2] = g_defaults) { }",
        );
        let ps = &func(&p, 0).params;
        let Param::Fixed(a) = &ps[0] else { panic!() };
        assert!(matches!(a.default, Some(ParamDefault::Expr(_))));
        let Param::Fixed(label) = &ps[1] else { panic!() };
        assert!(matches!(label.default, Some(ParamDefault::Array(Init::Expr(_)))));
        let Param::Fixed(data) = &ps[2] else { panic!() };
        assert!(matches!(data.default, Some(ParamDefault::Array(Init::List(_)))));
        let Param::Fixed(table) = &ps[3] else { panic!() };
        match table.default.as_ref().unwrap() {
            ParamDefault::Symbol(id) => assert_eq!(id.name, "g_defaults"),
            other => panic!("expected a symbol default, got {other:?}"),
        }
    }

    #[test]
    fn sizeof_and_tagof_defaults_name_another_parameter() {
        let p = parse_clean(
            "stock copy(dest[], len = sizeof dest, deep = sizeof grid[][], t = tagof(dest)) { }",
        );
        let ps = &func(&p, 0).params;
        let Param::Fixed(len) = &ps[1] else { panic!() };
        match len.default.as_ref().unwrap() {
            ParamDefault::SizeOf { arg, levels, .. } => {
                assert_eq!(arg.name, "dest");
                assert_eq!(*levels, 0);
            }
            other => panic!("expected sizeof, got {other:?}"),
        }
        let Param::Fixed(deep) = &ps[2] else { panic!() };
        match deep.default.as_ref().unwrap() {
            ParamDefault::SizeOf { arg, levels, .. } => {
                assert_eq!(arg.name, "grid");
                assert_eq!(*levels, 2, "each `[]` selects one sub-dimension");
            }
            other => panic!("expected sizeof, got {other:?}"),
        }
        let Param::Fixed(t) = &ps[3] else { panic!() };
        match t.default.as_ref().unwrap() {
            ParamDefault::TagOf { arg, .. } => assert_eq!(arg.name, "dest"),
            other => panic!("expected tagof, got {other:?}"),
        }
    }

    #[test]
    fn a_public_parameter_is_error_56() {
        let (_, d) = parse_src("stock f(@bad) { }");
        assert!(d.items().iter().any(|x| x.code == 56));
    }

    // --------------------------------------------------------------- pragmas

    #[test]
    fn a_pragma_keeps_its_tail_verbatim() {
        let p = parse_clean("#pragma library  Fun_Module\n#pragma semicolon 1\n");
        let Item::Pragma(lib) = &p.items[0] else { panic!("expected a pragma") };
        assert_eq!(lib.kind, PragmaKind::Library);
        assert_eq!(lib.args, "Fun_Module");
        let Item::Pragma(semi) = &p.items[1] else { panic!("expected a pragma") };
        assert_eq!(semi.kind, PragmaKind::Semicolon);
        assert_eq!(semi.args, "1");
    }

    #[test]
    fn an_unknown_pragma_is_warning_207_but_still_recorded() {
        let (p, d) = parse_src("#pragma nonsense whatever\n");
        assert!(d.items().iter().any(|x| x.code == 207));
        let Item::Pragma(pr) = &p.items[0] else { panic!("expected a pragma") };
        assert_eq!(pr.kind, PragmaKind::Unknown);
        assert_eq!(pr.args, "whatever");
    }

    #[test]
    fn other_directives_are_dropped_whole() {
        let p = parse_clean("#include <amxmodx>\nnew g;\n");
        assert_eq!(p.items.len(), 1, "`#include <amxmodx>` produces no item");
    }

    // -------------------------------------------------------------- recovery

    #[test]
    fn a_stray_brace_is_reported_once_and_parsing_continues() {
        let (p, d) = parse_src("}\nnew g_after;");
        assert!(d.items().iter().any(|x| x.code == 54));
        assert_eq!(d.error_count(), 1, "one bad token, one diagnostic");
        assert!(matches!(p.items[1], Item::Var(_)));
    }

    #[test]
    fn a_body_without_a_header_is_error_55_and_is_skipped_whole() {
        let (p, d) = parse_src("{ new inside; }\nnew g_after;");
        assert!(d.items().iter().any(|x| x.code == 55));
        assert_eq!(d.error_count(), 1);
        assert!(matches!(p.items[1], Item::Var(_)));
    }

    #[test]
    fn a_bad_declaration_does_not_truncate_the_file() {
        let (p, d) = parse_src("new 42;\nnew g_after;\n");
        assert!(d.items().iter().any(|x| x.code == 20));
        assert!(
            p.items.iter().any(|i| matches!(i, Item::Var(v)
                if v.declarators.iter().any(|dd| dd.name.name == "g_after"))),
            "the following declaration must still be parsed"
        );
    }

    #[test]
    fn a_missing_paren_in_a_header_is_reported_and_recovered() {
        let (_, d) = parse_src("stock f(a { }\nnew g_after;");
        assert!(d.error_count() >= 1);
    }

    // --------------------------------------------------------------- fixture

    #[test]
    fn the_declaration_fixture_parses_without_spurious_diagnostics() {
        let src = include_str!("../../zpc/tests/fixtures/decl_edge_cases.sma");
        let (program, diags) = parse_src(src);
        let shown: Vec<_> = diags.items().iter().map(|d| (d.code, d.message.clone())).collect();
        assert!(shown.is_empty(), "unexpected diagnostics: {shown:?}");
        assert!(
            !program.items.iter().any(|i| matches!(i, Item::Error(_))),
            "the fixture must contain no poisoned declarations"
        );

        let enums = program.items.iter().filter(|i| matches!(i, Item::Enum(_))).count();
        assert_eq!(enums, 4, "four enums: tagged, stepped, anonymous and struct-like");
        let funcs: Vec<_> = program
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Func(f) => Some(f),
                _ => None,
            })
            .collect();
        assert_eq!(funcs.len(), 7);
        assert_eq!(funcs[0].kind, FuncKind::Forward);
        assert_eq!(funcs[1].kind, FuncKind::Native);
        // stock Float:compute(id, Float:scale = 1.0, &result = 0, const label[] = "", ...)
        let compute = funcs[2];
        assert!(compute.modifiers.stock);
        assert_eq!(compute.return_tag.as_ref().unwrap().name.name, "Float");
        assert_eq!(compute.params.len(), 5);
        assert!(matches!(compute.params[4], Param::Rest(_)));
        assert!(compute.body.is_some());
        assert!(funcs[4].modifiers.static_ && funcs[4].modifiers.stock);
        assert!(matches!(funcs[5].name, FuncName::Operator { op: OverloadableOp::Add, .. }));
    }
}
