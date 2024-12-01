use super::{ArgParseError, CommandFormat};

use zip::CompressionMethod;

use std::{collections::VecDeque, ffi::OsString, mem, path::PathBuf};

#[derive(Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ContentTransform {
    Extract { name: Option<String> },
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Copy, Clone)]
pub enum ComponentSelector {
    #[default]
    Path,
    Basename,
    Dirname,
    FileExtension,
}

impl ComponentSelector {
    pub fn parse(s: &[u8]) -> Option<Self> {
        match s {
            b"path" => Some(Self::Path),
            b"basename" => Some(Self::Basename),
            b"dirname" => Some(Self::Dirname),
            b"ext" => Some(Self::FileExtension),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Copy, Clone)]
pub enum PatternSelectorType {
    Glob,
    Literal,
    Regexp,
}

impl PatternSelectorType {
    pub fn parse(s: &[u8]) -> Option<Self> {
        match s {
            b"glob" => Some(Self::Glob),
            b"lit" => Some(Self::Literal),
            b"rx" => Some(Self::Regexp),
            _ => None,
        }
    }

    const fn help_description(self) -> &'static str {
        match self {
            Self::Glob => "glob",
            Self::Literal => "literal",
            Self::Regexp => "regexp",
        }
    }

    const fn arg_abbreviation(self) -> &'static str {
        match self {
            Self::Glob => "glob",
            Self::Literal => "lit",
            Self::Regexp => "rx",
        }
    }

    fn generate_match_help_text(self) -> String {
        format!(
            r#"These flags default to interpreting a <pattern> argument as a {} string to
match against the entire entry name, which can be explicitly requested as
follows:

   --match=path:{} <pattern>"#,
            self.help_description(),
            self.arg_abbreviation(),
        )
    }

    pub fn generate_match_default_help_text() -> String {
        Self::default_for_match().generate_match_help_text()
    }
}

#[derive(Copy, Clone)]
pub enum PatSelContext {
    MatchOnly,
    MatchAndTransform,
}

impl PatSelContext {
    #[allow(dead_code)]
    const fn first_default(self) -> &'static str {
        match self {
            Self::MatchOnly => "[DEFAULT] ",
            Self::MatchAndTransform => "[DEFAULT for matching] ",
        }
    }

    #[allow(dead_code)]
    const fn second_default(self) -> &'static str {
        match self {
            Self::MatchOnly => "",
            Self::MatchAndTransform => "[DEFAULT for replacement] ",
        }
    }
}

#[cfg(all(feature = "glob", feature = "rx"))]
impl PatternSelectorType {
    pub fn generate_pat_sel_help_section(ctx: PatSelContext) -> String {
        format!(
            r#"pat-sel  = glob		{}(interpret as a shell glob)
         = lit		(interpret as literal string)
         = rx		{}(interpret as a regular expression)
         = <pat-sel><pat-mod...>
                        (apply search modifiers from <pat-mod>)"#,
            ctx.first_default(),
            ctx.second_default(),
        )
    }
}

#[cfg(all(feature = "glob", not(feature = "rx")))]
impl PatternSelectorType {
    pub fn generate_pat_sel_help_section(ctx: PatSelContext) -> String {
        format!(
            r#"pat-sel  = glob		{}(interpret as a shell glob)
         = lit		{}(interpret as literal string)
         = <pat-sel><pat-mod...>
                        (apply search modifiers from <pat-mod>)"#,
            ctx.first_default(),
            ctx.second_default(),
        )
    }
}

#[cfg(all(not(feature = "glob"), feature = "rx"))]
impl PatternSelectorType {
    pub fn generate_pat_sel_help_section(ctx: PatSelContext) -> String {
        format!(
            r#"pat-sel  = lit		{}(interpret as literal string)
         = rx		{}(interpret as a regular expression)
         = <pat-sel><pat-mod...>
                        (apply search modifiers from <pat-mod>)"#,
            ctx.first_default(),
            ctx.second_default(),
        )
    }
}

#[cfg(not(any(feature = "glob", feature = "rx")))]
impl PatternSelectorType {
    pub fn generate_pat_sel_help_section(_ctx: PatSelContext) -> String {
        r#"pat-sel  = lit		[DEFAULT] (interpret as literal string)
         = <pat-sel><pat-mod...>
                        (apply search modifiers from <pat-mod>)"#
            .to_string()
    }
}

#[cfg(feature = "glob")]
impl PatternSelectorType {
    pub const fn default_for_match() -> Self {
        Self::Glob
    }

    pub const fn generate_glob_replacement_note(ctx: PatSelContext) -> &'static str {
        match ctx {
            PatSelContext::MatchOnly => "",
            PatSelContext::MatchAndTransform => {
                "\n*Note:* glob patterns are not supported for replacement, and attempting to use
them with e.g '--transform:glob' will produce an error.\n"
            }
        }
    }
}

#[cfg(not(feature = "glob"))]
impl PatternSelectorType {
    pub const fn default_for_match() -> Self {
        Self::Literal
    }

    pub const fn generate_glob_replacement_note(_ctx: PatSelContext) -> &'static str {
        ""
    }
}

#[cfg(feature = "rx")]
impl PatternSelectorType {
    pub const fn default_for_replacement() -> Self {
        Self::Regexp
    }
}

#[cfg(not(feature = "rx"))]
impl PatternSelectorType {
    pub const fn default_for_replacement() -> Self {
        Self::Literal
    }
}

#[derive(Debug)]
pub enum PatternSelectorModifier {
    CaseInsensitive,
    MultipleMatches,
    PrefixAnchored,
    SuffixAnchored,
}

impl PatternSelectorModifier {
    pub fn parse(s: &[u8]) -> Option<Self> {
        match s {
            b"i" => Some(Self::CaseInsensitive),
            b"g" => Some(Self::MultipleMatches),
            b"p" => Some(Self::PrefixAnchored),
            b"s" => Some(Self::SuffixAnchored),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PatternModifierFlags {
    pub case_insensitive: bool,
    pub multiple_matches: bool,
    pub prefix_anchored: bool,
    pub suffix_anchored: bool,
}

#[derive(Debug)]
pub struct PatternSelector {
    pub pat_sel: PatternSelectorType,
    pub modifiers: PatternModifierFlags,
}

impl PatternSelector {
    pub fn parse(s: &[u8]) -> Option<Self> {
        match s.iter().position(|c| *c == b':') {
            Some(modifiers_ind) => {
                let pat_sel_str = &s[..modifiers_ind];
                let modifiers_str = &s[(modifiers_ind + 1)..];

                let pat_sel = PatternSelectorType::parse(pat_sel_str)?;

                let mut modifiers = PatternModifierFlags::default();
                let mod_els = modifiers_str
                    .split(|c| *c == b':')
                    .map(PatternSelectorModifier::parse)
                    .collect::<Option<Vec<_>>>()?;
                for m in mod_els.into_iter() {
                    match m {
                        PatternSelectorModifier::CaseInsensitive => {
                            modifiers.case_insensitive = true;
                        }
                        PatternSelectorModifier::MultipleMatches => {
                            modifiers.multiple_matches = true;
                        }
                        PatternSelectorModifier::PrefixAnchored => {
                            modifiers.prefix_anchored = true;
                        }
                        PatternSelectorModifier::SuffixAnchored => {
                            modifiers.suffix_anchored = true;
                        }
                    }
                }
                Some(Self { pat_sel, modifiers })
            }
            None => {
                let pat_sel = PatternSelectorType::parse(s)?;
                Some(Self {
                    pat_sel,
                    modifiers: Default::default(),
                })
            }
        }
    }

    pub fn default_for_context(ctx: PatternContext) -> Self {
        match ctx {
            PatternContext::Match => Self::default_for_match(),
            PatternContext::Replacement => Self::default_for_replacement(),
        }
    }

    pub fn default_for_match() -> Self {
        Self {
            pat_sel: PatternSelectorType::default_for_match(),
            modifiers: PatternModifierFlags::default(),
        }
    }

    pub fn default_for_replacement() -> Self {
        Self {
            pat_sel: PatternSelectorType::default_for_replacement(),
            modifiers: PatternModifierFlags::default(),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PatternContext {
    Match,
    Replacement,
}

pub fn parse_only_pat_sel(s: &[u8], ctx: PatternContext) -> Option<PatternSelector> {
    match s.iter().position(|c| *c == b':') {
        Some(pat_sel_ind) => {
            let pat_sel_str = &s[(pat_sel_ind + 1)..];

            let pat_sel = PatternSelector::parse(pat_sel_str)?;
            Some(pat_sel)
        }
        None => Some(PatternSelector::default_for_context(ctx)),
    }
}

pub fn parse_comp_and_pat_sel(
    s: &[u8],
    ctx: PatternContext,
) -> Option<(ComponentSelector, PatternSelector)> {
    match (
        s.iter().position(|c| *c == b'='),
        s.iter().position(|c| *c == b':'),
    ) {
        (Some(comp_sel_ind), Some(pat_sel_ind)) => {
            if comp_sel_ind >= pat_sel_ind {
                return None;
            }
            let comp_sel_str = &s[(comp_sel_ind + 1)..pat_sel_ind];
            let pat_sel_str = &s[(pat_sel_ind + 1)..];

            let comp_sel = ComponentSelector::parse(comp_sel_str)?;
            let pat_sel = PatternSelector::parse(pat_sel_str)?;
            Some((comp_sel, pat_sel))
        }
        (Some(comp_sel_ind), None) => {
            let comp_sel_str = &s[(comp_sel_ind + 1)..];

            let comp_sel = ComponentSelector::parse(comp_sel_str)?;
            let pat_sel = PatternSelector::default_for_context(ctx);
            Some((comp_sel, pat_sel))
        }
        (None, Some(pat_sel_ind)) => {
            let pat_sel_str = &s[(pat_sel_ind + 1)..];

            let pat_sel = PatternSelector::parse(pat_sel_str)?;
            let comp_sel = ComponentSelector::default();
            Some((comp_sel, pat_sel))
        }
        (None, None) => {
            let comp_sel = ComponentSelector::default();
            let pat_sel = PatternSelector::default_for_context(ctx);
            Some((comp_sel, pat_sel))
        }
    }
}

#[derive(Debug)]
pub enum EntryType {
    File,
    Dir,
    Symlink,
}

impl EntryType {
    pub fn parse(s: &[u8]) -> Option<Self> {
        match s {
            b"file" => Some(Self::File),
            b"dir" => Some(Self::Dir),
            b"symlink" => Some(Self::Symlink),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum NonSpecificCompressionMethodArg {
    Any,
    Known,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum SpecificCompressionMethodArg {
    Stored,
    Deflated,
    #[cfg(feature = "deflate64")]
    Deflate64,
    #[cfg(feature = "bzip2")]
    Bzip2,
    #[cfg(feature = "zstd")]
    Zstd,
    #[cfg(feature = "lzma")]
    Lzma,
    #[cfg(feature = "xz")]
    Xz,
}

impl SpecificCompressionMethodArg {
    pub const KNOWN_COMPRESSION_METHODS: &[CompressionMethod] = &[
        CompressionMethod::Stored,
        CompressionMethod::Deflated,
        #[cfg(feature = "deflate64")]
        CompressionMethod::Deflate64,
        #[cfg(feature = "bzip2")]
        CompressionMethod::Bzip2,
        #[cfg(feature = "zstd")]
        CompressionMethod::Zstd,
        #[cfg(feature = "lzma")]
        CompressionMethod::Lzma,
        #[cfg(feature = "xz")]
        CompressionMethod::Xz,
    ];

    pub fn translate_to_zip(self) -> CompressionMethod {
        match self {
            Self::Stored => CompressionMethod::Stored,
            Self::Deflated => CompressionMethod::Deflated,
            #[cfg(feature = "deflate64")]
            Self::Deflate64 => CompressionMethod::Deflate64,
            #[cfg(feature = "bzip2")]
            Self::Bzip2 => CompressionMethod::Bzip2,
            #[cfg(feature = "zstd")]
            Self::Zstd => CompressionMethod::Zstd,
            #[cfg(feature = "lzma")]
            Self::Lzma => CompressionMethod::Lzma,
            #[cfg(feature = "xz")]
            Self::Xz => CompressionMethod::Xz,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompressionMethodArg {
    NonSpecific(NonSpecificCompressionMethodArg),
    Specific(SpecificCompressionMethodArg),
}

impl CompressionMethodArg {
    pub fn parse(s: &[u8]) -> Option<Self> {
        match s {
            b"any" => Some(Self::NonSpecific(NonSpecificCompressionMethodArg::Any)),
            b"known" => Some(Self::NonSpecific(NonSpecificCompressionMethodArg::Known)),
            b"stored" => Some(Self::Specific(SpecificCompressionMethodArg::Stored)),
            b"deflated" => Some(Self::Specific(SpecificCompressionMethodArg::Deflated)),
            #[cfg(feature = "deflate64")]
            b"deflate64" => Some(Self::Specific(SpecificCompressionMethodArg::Deflate64)),
            #[cfg(feature = "bzip2")]
            b"bzip2" => Some(Self::Specific(SpecificCompressionMethodArg::Bzip2)),
            #[cfg(feature = "zstd")]
            b"zstd" => Some(Self::Specific(SpecificCompressionMethodArg::Zstd)),
            #[cfg(feature = "lzma")]
            b"lzma" => Some(Self::Specific(SpecificCompressionMethodArg::Lzma)),
            #[cfg(feature = "xz")]
            b"xz" => Some(Self::Specific(SpecificCompressionMethodArg::Xz)),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum DepthLimitArg {
    Max(u8),
    Min(u8),
}

#[derive(Debug)]
pub enum SizeArg {
    Max(u64),
    Min(u64),
}

#[derive(Debug)]
pub struct MatchArg {
    pub comp_sel: ComponentSelector,
    pub pat_sel: PatternSelector,
    pub pattern: String,
}

#[derive(Debug)]
pub enum TrivialPredicate {
    True,
    False,
}

#[derive(Debug)]
pub enum Predicate {
    Trivial(TrivialPredicate),
    EntryType(EntryType),
    CompressionMethod(CompressionMethodArg),
    DepthLimit(DepthLimitArg),
    Size(SizeArg),
    Match(MatchArg),
}

#[derive(Debug)]
enum ExprOp {
    Negation,
    And,
    Or,
}

#[derive(Debug)]
enum ExprArg {
    PrimitivePredicate(Predicate),
    Op(ExprOp),
    Subgroup(MatchExpression),
}

#[derive(Debug, Default)]
struct SingleExprLevel {
    expr_args: Vec<ExprArg>,
}

impl SingleExprLevel {
    pub fn push_arg(&mut self, arg: ExprArg) {
        self.expr_args.push(arg);
    }

    fn get_negation(expr_args: &mut VecDeque<ExprArg>) -> Result<MatchExpression, ArgParseError> {
        let negated_expr: MatchExpression = match expr_args.pop_front().ok_or_else(|| {
            Extract::exit_arg_invalid(&format!(
                "negation was only expression in list inside match expr (rest: {expr_args:?})"
            ))
        })? {
            ExprArg::Subgroup(match_expr) => {
                /* We have a valid match expression, so just negate it without
                 * wrapping. */
                MatchExpression::Negated(Box::new(match_expr))
            }
            ExprArg::PrimitivePredicate(predicate) => {
                /* We got a primitive predicate, so just negate it! */
                MatchExpression::Negated(Box::new(MatchExpression::PrimitivePredicate(predicate)))
            }
            ExprArg::Op(op) => {
                /* Negation before any other operator is invalid. */
                return Err(Extract::exit_arg_invalid(&format!(
                        "negation before operator {op:?} inside match expr is invalid (rest: {expr_args:?})"
                    )));
            }
        };
        Ok(negated_expr)
    }

    fn get_non_operator(
        expr_args: &mut VecDeque<ExprArg>,
    ) -> Result<MatchExpression, ArgParseError> {
        let next_expr: MatchExpression = match expr_args.pop_front().ok_or_else(|| {
            /* We can't fold an empty list. */
            Extract::exit_arg_invalid(&format!(
                "empty expression list inside match expr (rest: {expr_args:?})"
            ))
        })? {
            /* This is already an evaluated match expression, so just start with that. */
            ExprArg::Subgroup(match_expr) => match_expr,
            ExprArg::PrimitivePredicate(predicate) => {
                /* Success! We start with a simple predicate. */
                MatchExpression::PrimitivePredicate(predicate)
            }
            ExprArg::Op(op) => match op {
                /* We started with negation, which means we need to get the next arg to resolve
                 * it. */
                ExprOp::Negation => Self::get_negation(expr_args)?,
                /* Starting with a binary operator is invalid. */
                op @ (ExprOp::And | ExprOp::Or) => {
                    return Err(Extract::exit_arg_invalid(&format!(
                            "expression list cannot begin with binary operator {op:?} (rest: {expr_args:?})"
                        )));
                }
            },
        };
        Ok(next_expr)
    }

    pub fn fold(self) -> Result<MatchExpression, ArgParseError> {
        let Self { expr_args } = self;
        let mut expr_args: VecDeque<_> = expr_args.into();

        /* Get a valid match expression to start our fold with. */
        let mut cur_expr: MatchExpression = Self::get_non_operator(&mut expr_args)?;

        /* Now fold the expression rightwards! */
        while let Some(next_arg) = expr_args.pop_front() {
            match next_arg {
                /* Implicit AND, wrapping the primitive result into a match. */
                ExprArg::PrimitivePredicate(predicate) => {
                    let next_expr = MatchExpression::PrimitivePredicate(predicate);
                    cur_expr = MatchExpression::And {
                        explicit: false,
                        left: Box::new(cur_expr),
                        right: Box::new(next_expr),
                    };
                }
                /* Implicit AND, without needing to wrap the result. */
                ExprArg::Subgroup(match_expr) => {
                    cur_expr = MatchExpression::And {
                        explicit: false,
                        left: Box::new(cur_expr),
                        right: Box::new(match_expr),
                    };
                }
                /* Evaluate the operator according to association. */
                ExprArg::Op(op) => match op {
                    /* Negation applies to the next element, so retrieve it! */
                    ExprOp::Negation => {
                        let next_expr = Self::get_negation(&mut expr_args)?;
                        cur_expr = MatchExpression::And {
                            explicit: false,
                            left: Box::new(cur_expr),
                            right: Box::new(next_expr),
                        };
                    }
                    /* Explicit AND requires the next element. */
                    ExprOp::And => {
                        let next_expr = Self::get_non_operator(&mut expr_args)?;
                        cur_expr = MatchExpression::And {
                            explicit: true,
                            left: Box::new(cur_expr),
                            right: Box::new(next_expr),
                        };
                    }
                    /* OR requires the next element. */
                    ExprOp::Or => {
                        let next_expr = Self::get_non_operator(&mut expr_args)?;
                        cur_expr = MatchExpression::Or {
                            left: Box::new(cur_expr),
                            right: Box::new(next_expr),
                        };
                    }
                },
            }
        }

        assert!(expr_args.is_empty());
        Ok(cur_expr)
    }
}

#[derive(Debug)]
pub enum MatchExpression {
    PrimitivePredicate(Predicate),
    Negated(Box<MatchExpression>),
    And {
        explicit: bool,
        left: Box<MatchExpression>,
        right: Box<MatchExpression>,
    },
    Or {
        left: Box<MatchExpression>,
        right: Box<MatchExpression>,
    },
    Grouped(Box<MatchExpression>),
}

impl MatchExpression {
    pub fn parse_argv<C: CommandFormat>(
        argv: &mut VecDeque<OsString>,
    ) -> Result<Self, ArgParseError> {
        let mut expr_stack: Vec<SingleExprLevel> = Vec::new();
        let mut top_exprs = SingleExprLevel::default();

        while let Some(arg) = argv.pop_front() {
            match arg.as_encoded_bytes() {
                /* Parse primitive predicates. */
                b"-true" => {
                    top_exprs.push_arg(ExprArg::PrimitivePredicate(Predicate::Trivial(
                        TrivialPredicate::True,
                    )));
                }
                b"-false" => {
                    top_exprs.push_arg(ExprArg::PrimitivePredicate(Predicate::Trivial(
                        TrivialPredicate::False,
                    )));
                }
                b"-t" | b"--type" => {
                    let type_arg = argv
                        .pop_front()
                        .ok_or_else(|| C::exit_arg_invalid("no argument provided for -t/--type"))?;
                    let entry_type =
                        EntryType::parse(type_arg.as_encoded_bytes()).ok_or_else(|| {
                            C::exit_arg_invalid(&format!("invalid --type argument: {type_arg:?}"))
                        })?;
                    top_exprs.push_arg(ExprArg::PrimitivePredicate(Predicate::EntryType(
                        entry_type,
                    )));
                }
                b"--compression-method" => {
                    let method_arg = argv.pop_front().ok_or_else(|| {
                        C::exit_arg_invalid("no argument provided for --compression-method")
                    })?;
                    let method = CompressionMethodArg::parse(method_arg.as_encoded_bytes())
                        .ok_or_else(|| {
                            C::exit_arg_invalid(&format!(
                                "invalid --compression-method argument: {method_arg:?}"
                            ))
                        })?;
                    top_exprs.push_arg(ExprArg::PrimitivePredicate(Predicate::CompressionMethod(
                        method,
                    )));
                }
                b"--max-depth" => {
                    let max_depth: u8 = argv
                        .pop_front()
                        .ok_or_else(|| C::exit_arg_invalid("no argument provided for --max-depth"))?
                        .into_string()
                        .map_err(|depth_arg| {
                            C::exit_arg_invalid(&format!(
                                "invalid unicode provided for --max-depth: {depth_arg:?}"
                            ))
                        })?
                        .parse::<u8>()
                        .map_err(|e| {
                            C::exit_arg_invalid(&format!(
                                "failed to parse --max-depth arg as u8: {e:?}"
                            ))
                        })?;
                    top_exprs.push_arg(ExprArg::PrimitivePredicate(Predicate::DepthLimit(
                        DepthLimitArg::Max(max_depth),
                    )));
                }
                b"--min-depth" => {
                    let min_depth: u8 = argv
                        .pop_front()
                        .ok_or_else(|| C::exit_arg_invalid("no argument provided for --min-depth"))?
                        .into_string()
                        .map_err(|depth_arg| {
                            C::exit_arg_invalid(&format!(
                                "invalid unicode provided for --min-depth: {depth_arg:?}"
                            ))
                        })?
                        .parse::<u8>()
                        .map_err(|e| {
                            C::exit_arg_invalid(&format!(
                                "failed to parse --min-depth arg as u8: {e:?}"
                            ))
                        })?;
                    top_exprs.push_arg(ExprArg::PrimitivePredicate(Predicate::DepthLimit(
                        DepthLimitArg::Min(min_depth),
                    )));
                }
                b"--max-size" => {
                    let max_size: u64 = argv
                        .pop_front()
                        .ok_or_else(|| C::exit_arg_invalid("no argument provided for --max-size"))?
                        .into_string()
                        .map_err(|size_arg| {
                            C::exit_arg_invalid(&format!(
                                "invalid unicode provided for --max-size: {size_arg:?}"
                            ))
                        })?
                        .parse::<u64>()
                        .map_err(|e| {
                            C::exit_arg_invalid(&format!(
                                "failed to parse --max-size arg as u64: {e:?}"
                            ))
                        })?;
                    top_exprs.push_arg(ExprArg::PrimitivePredicate(Predicate::Size(SizeArg::Max(
                        max_size,
                    ))));
                }
                b"--min-size" => {
                    let min_size: u64 = argv
                        .pop_front()
                        .ok_or_else(|| C::exit_arg_invalid("no argument provided for --min-size"))?
                        .into_string()
                        .map_err(|size_arg| {
                            C::exit_arg_invalid(&format!(
                                "invalid unicode provided for --min-size: {size_arg:?}"
                            ))
                        })?
                        .parse::<u64>()
                        .map_err(|e| {
                            C::exit_arg_invalid(&format!(
                                "failed to parse --min-size arg as u64: {e:?}"
                            ))
                        })?;
                    top_exprs.push_arg(ExprArg::PrimitivePredicate(Predicate::Size(SizeArg::Min(
                        min_size,
                    ))));
                }
                b"-m" => {
                    let pattern: String = argv
                        .pop_front()
                        .ok_or_else(|| C::exit_arg_invalid("no argument provided for -m"))?
                        .into_string()
                        .map_err(|pattern| {
                            C::exit_arg_invalid(&format!(
                                "invalid unicode provided for -m: {pattern:?}"
                            ))
                        })?;
                    let comp_sel = ComponentSelector::default();
                    let pat_sel = PatternSelector::default_for_context(PatternContext::Match);
                    top_exprs.push_arg(ExprArg::PrimitivePredicate(Predicate::Match(MatchArg {
                        comp_sel,
                        pat_sel,
                        pattern,
                    })));
                }
                arg_bytes if arg_bytes.starts_with(b"--match") => {
                    let (comp_sel, pat_sel) = parse_comp_and_pat_sel(
                        arg_bytes,
                        PatternContext::Match,
                    )
                    .ok_or_else(|| {
                        C::exit_arg_invalid(&format!("invalid --match argument modifiers: {arg:?}"))
                    })?;
                    let pattern: String = argv
                        .pop_front()
                        .ok_or_else(|| C::exit_arg_invalid("no argument provided for --match"))?
                        .into_string()
                        .map_err(|pattern| {
                            C::exit_arg_invalid(&format!(
                                "invalid unicode provided for --match: {pattern:?}"
                            ))
                        })?;
                    top_exprs.push_arg(ExprArg::PrimitivePredicate(Predicate::Match(MatchArg {
                        comp_sel,
                        pat_sel,
                        pattern,
                    })));
                }

                /* Parse operators. */
                b"!" | b"-not" => {
                    top_exprs.push_arg(ExprArg::Op(ExprOp::Negation));
                }
                b"&" | b"-and" => {
                    top_exprs.push_arg(ExprArg::Op(ExprOp::And));
                }
                b"|" | b"-or" => {
                    top_exprs.push_arg(ExprArg::Op(ExprOp::Or));
                }

                /* Process groups with stack logic! */
                b"(" | b"-open" => {
                    expr_stack.push(mem::take(&mut top_exprs));
                }
                b")" | b"-close" => {
                    /* Get the unevaluated exprs from the previous nesting level. */
                    let prev_level = expr_stack.pop().ok_or_else(|| {
                        C::exit_arg_invalid("too many close parens inside match expr")
                    })?;
                    /* Move the previous nesting level into current, and evaluate the current
                     * nesting level. */
                    let group_expr = mem::replace(&mut top_exprs, prev_level).fold()?;
                    /* Wrap the completed group in a Grouped. */
                    let group_expr = MatchExpression::Grouped(Box::new(group_expr));
                    /* Push the completed and evaluated group into the current nesting level. */
                    top_exprs.push_arg(ExprArg::Subgroup(group_expr));
                }

                /* Conclude the match expr processing. */
                b"--expr" => {
                    break;
                }
                _ => {
                    return Err(C::exit_arg_invalid(&format!(
                            "unrecognized match expression component {arg:?}: all match expressions must start and end with a --expr flag"
                        )));
                }
            }
        }

        if !expr_stack.is_empty() {
            return Err(C::exit_arg_invalid(
                "not enough close parens inside match expr",
            ));
        }
        top_exprs.fold()
    }
}

#[derive(Debug)]
pub enum TrivialTransform {
    Identity,
}

#[derive(Debug)]
pub enum BasicTransform {
    StripComponents(u8),
    AddPrefix(String),
}

#[derive(Debug)]
pub struct TransformArg {
    pub comp_sel: ComponentSelector,
    pub pat_sel: PatternSelector,
    pub pattern: String,
    pub replacement_spec: String,
}

#[derive(Debug)]
pub enum ComplexTransform {
    Transform(TransformArg),
}

#[derive(Debug)]
pub enum NameTransform {
    Trivial(TrivialTransform),
    Basic(BasicTransform),
    Complex(ComplexTransform),
}

#[derive(Debug)]
enum ExtractArg {
    Match(MatchExpression),
    NameTransform(NameTransform),
    ContentTransform(ContentTransform),
}

#[derive(Debug)]
pub struct EntrySpec {
    pub match_expr: Option<MatchExpression>,
    pub name_transforms: Vec<NameTransform>,
    pub content_transform: ContentTransform,
}

impl EntrySpec {
    fn parse_extract_args(
        args: impl IntoIterator<Item = ExtractArg>,
    ) -> Result<Vec<Self>, ArgParseError> {
        let mut match_expr: Option<MatchExpression> = None;
        let mut name_transforms: Vec<NameTransform> = Vec::new();

        let mut ret: Vec<Self> = Vec::new();

        for arg in args.into_iter() {
            match arg {
                ExtractArg::Match(new_expr) => {
                    if let Some(prev_expr) = match_expr.take() {
                        return Err(Extract::exit_arg_invalid(&format!(
                                "more than one match expr was provided for the same entry: {prev_expr:?} and {new_expr:?}"
                            )));
                    }
                    match_expr = Some(new_expr);
                }
                ExtractArg::NameTransform(n_trans) => {
                    name_transforms.push(n_trans);
                }
                ExtractArg::ContentTransform(c_trans) => {
                    let spec = Self {
                        match_expr: match_expr.take(),
                        name_transforms: mem::take(&mut name_transforms),
                        content_transform: c_trans,
                    };
                    ret.push(spec);
                }
            }
        }
        if let Some(match_expr) = match_expr {
            return Err(Extract::exit_arg_invalid(&format!(
                "match expr {match_expr:?} was provided with no corresponding content \
transform. add -x/--extract to construct a complete entry spec"
            )));
        }
        if !name_transforms.is_empty() {
            return Err(Extract::exit_arg_invalid(&format!(
                "name transforms {name_transforms:?} were provided with no corresponding \
content transform. add -x/--extract to construct a complete entry spec"
            )));
        }

        Ok(ret)
    }
}

#[derive(Debug)]
pub enum OutputCollation {
    ConcatenateStdout,
    ConcatenateFile { path: PathBuf, append: bool },
    Filesystem { output_dir: PathBuf, mkdir: bool },
}

#[derive(Debug)]
pub struct NamedOutput {
    pub name: String,
    pub output: OutputCollation,
}

#[derive(Debug)]
pub struct OutputSpecs {
    pub default: Option<OutputCollation>,
    pub named: Vec<NamedOutput>,
}

impl Default for OutputSpecs {
    fn default() -> Self {
        Self {
            default: Some(OutputCollation::Filesystem {
                output_dir: PathBuf::from("."),
                mkdir: false,
            }),
            named: Vec::new(),
        }
    }
}

impl OutputSpecs {
    pub fn parse_argv(argv: &mut VecDeque<OsString>) -> Result<Self, ArgParseError> {
        let mut default: Option<OutputCollation> = None;
        let mut named: Vec<NamedOutput> = Vec::new();
        let mut cur_name: Option<String> = None;

        while let Some(arg) = argv.pop_front() {
            match arg.as_encoded_bytes() {
                b"-h" | b"--help" => {
                    let help_text = Extract::generate_full_help_text();
                    return Err(ArgParseError::StdoutMessage(help_text));
                }
                b"--name" => {
                    let name = argv
                        .pop_front()
                        .ok_or_else(|| {
                            Extract::exit_arg_invalid("no argument provided for --name")
                        })?
                        .into_string()
                        .map_err(|name| {
                            Extract::exit_arg_invalid(&format!(
                                "invalid unicode provided for --name: {name:?}"
                            ))
                        })?;
                    if let Some(prev_name) = cur_name.take() {
                        return Err(Extract::exit_arg_invalid(&format!(
                            "multiple names provided for output: {prev_name:?} and {name:?}"
                        )));
                    }
                    cur_name = Some(name);
                }
                b"-d" => {
                    let dir_path = argv
                        .pop_front()
                        .map(PathBuf::from)
                        .ok_or_else(|| Extract::exit_arg_invalid("no argument provided for -d"))?;
                    let output = OutputCollation::Filesystem {
                        output_dir: dir_path,
                        mkdir: false,
                    };
                    if let Some(name) = cur_name.take() {
                        named.push(NamedOutput { name, output });
                    } else if let Some(default) = default.take() {
                        return Err(Extract::exit_arg_invalid(&format!(
                            "multiple unnamed outputs provided: {default:?} and {output:?}"
                        )));
                    } else {
                        default = Some(output);
                    }
                }
                arg_bytes if arg_bytes.starts_with(b"--output-directory") => {
                    let mkdir = match arg_bytes {
                        b"--output-directory" => false,
                        b"--output-directory:mkdir" => true,
                        _ => {
                            return Err(Extract::exit_arg_invalid(&format!(
                                "invalid suffix provided to --output-directory: {arg:?}"
                            )));
                        }
                    };
                    let dir_path = argv.pop_front().map(PathBuf::from).ok_or_else(|| {
                        Extract::exit_arg_invalid("no argument provided for --output-directory")
                    })?;
                    let output = OutputCollation::Filesystem {
                        output_dir: dir_path,
                        mkdir,
                    };
                    if let Some(name) = cur_name.take() {
                        named.push(NamedOutput { name, output });
                    } else if let Some(default) = default.take() {
                        return Err(Extract::exit_arg_invalid(&format!(
                            "multiple unnamed outputs provided: {default:?} and {output:?}"
                        )));
                    } else {
                        default = Some(output);
                    }
                }
                b"--stdout" => {
                    let output = OutputCollation::ConcatenateStdout;
                    if let Some(name) = cur_name.take() {
                        named.push(NamedOutput { name, output });
                    } else if let Some(default) = default.take() {
                        return Err(Extract::exit_arg_invalid(&format!(
                            "multiple unnamed outputs provided: {default:?} and {output:?}"
                        )));
                    } else {
                        default = Some(output);
                    }
                }
                b"-f" => {
                    let file_path = argv
                        .pop_front()
                        .map(PathBuf::from)
                        .ok_or_else(|| Extract::exit_arg_invalid("no argument provided for -f"))?;
                    let output = OutputCollation::ConcatenateFile {
                        path: file_path,
                        append: false,
                    };
                    if let Some(name) = cur_name.take() {
                        named.push(NamedOutput { name, output });
                    } else if let Some(default) = default.take() {
                        return Err(Extract::exit_arg_invalid(&format!(
                            "multiple unnamed outputs provided: {default:?} and {output:?}"
                        )));
                    } else {
                        default = Some(output);
                    }
                }
                arg_bytes if arg_bytes.starts_with(b"--output-file") => {
                    let append = match arg_bytes {
                        b"--output-file" => false,
                        b"--output-file:append" => true,
                        _ => {
                            return Err(Extract::exit_arg_invalid(&format!(
                                "invalid suffix provided to --output-file: {arg:?}"
                            )));
                        }
                    };
                    let file_path = argv.pop_front().map(PathBuf::from).ok_or_else(|| {
                        Extract::exit_arg_invalid("no argument provided for --output-file")
                    })?;
                    let output = OutputCollation::ConcatenateFile {
                        path: file_path,
                        append,
                    };
                    if let Some(name) = cur_name.take() {
                        named.push(NamedOutput { name, output });
                    } else if let Some(default) = default.take() {
                        return Err(Extract::exit_arg_invalid(&format!(
                            "multiple unnamed outputs provided: {default:?} and {output:?}"
                        )));
                    } else {
                        default = Some(output);
                    }
                }
                _ => {
                    argv.push_front(arg);
                    break;
                }
            }
        }
        if let Some(name) = cur_name {
            return Err(Extract::exit_arg_invalid(&format!(
                "trailing --name argument provided without output spec: {name:?}"
            )));
        }

        Ok(if default.is_none() && named.is_empty() {
            Self::default()
        } else {
            Self { default, named }
        })
    }
}

#[derive(Debug)]
pub struct InputSpec {
    pub stdin_stream: bool,
    pub zip_paths: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct Extract {
    pub output_specs: OutputSpecs,
    pub entry_specs: Vec<EntrySpec>,
    pub input_spec: InputSpec,
}

impl Extract {
    #[cfg(feature = "deflate64")]
    const DEFLATE64_HELP_LINE: &'static str = "          - deflate64:\twith deflate64\n";
    #[cfg(not(feature = "deflate64"))]
    const DEFLATE64_HELP_LINE: &'static str = "";

    #[cfg(feature = "bzip2")]
    const BZIP2_HELP_LINE: &'static str = "          - bzip2:\twith bzip2\n";
    #[cfg(not(feature = "bzip2"))]
    const BZIP2_HELP_LINE: &'static str = "";

    #[cfg(feature = "zstd")]
    const ZSTD_HELP_LINE: &'static str = "          - zstd:\twith zstd\n";
    #[cfg(not(feature = "zstd"))]
    const ZSTD_HELP_LINE: &'static str = "";

    #[cfg(feature = "lzma")]
    const LZMA_HELP_LINE: &'static str = "          - lzma:\twith lzma\n";
    #[cfg(not(feature = "lzma"))]
    const LZMA_HELP_LINE: &'static str = "";

    #[cfg(feature = "xz")]
    const XZ_HELP_LINE: &'static str = "          - xz:\t\twith xz\n";
    #[cfg(not(feature = "xz"))]
    const XZ_HELP_LINE: &'static str = "";

    pub fn generate_match_expr_help_text() -> String {
        format!(
            r#"
## Match expressions (match-expr):

Entry matching logic composes boolean arithmetic expressions ("expr") in terms
of basic "predicates" which test some component of the zip entry. Expressions
can be composed as follows, in order of precedence:

expr = ( <expr> )	(grouping to force precedence)
     = ! <expr>		(negation)
     = <expr> & <expr>	(short-circuiting conjunction "and")
     = <expr> <expr>	(implicit &)
     = <expr> | <expr>	(disjunction "or")
     = <predicate>	(evaluate <predicate> on entry)

### Operators:
The operators to compose match expressions must be quoted in shell commands
(e.g. as \( or '('), so alternatives are provided which do not require
special quoting:

Grouping operators:
  (,  -open
  ),  -close

Unary operators:
  !,  -not

Binary operators:
  |,  -or
  &,  -and

### Predicates (predicate):
These arguments are interpreted as basic predicates, returning true or false in
response to a specific zip entry.

Trivial:
These results do not depend on the entry data at all:

      -true	Always return true.
      -false	Always return false.

If a match expression is not provided, it defaults to the behavior of -true.

Basic:
These results are dependent on the entry data:

  -t, --type [file|dir|symlink]
          Match entries of the given type.
          Note that directory entries may have specific mode bits set, or they may just be
          zero-length entries whose name ends in '/'.

      --compression-method <method-name>
          Match entries compressed with the given compression technique.

          Possible values:
          - any:	any compression method at all
          - known:	any compression method this binary is able to decompress
          - stored:	uncompressed
          - deflated:	with deflate
{}{}{}{}{}
          Using e.g. '--compression-method known' as a match expression filters
          entries to only those which can be successfully decompressed.

      --max-depth <num>
          Match entries with at *most* <num> components of their
          containing directory.
      --min-depth <num>
          Match entries with at *least* <num> components of their
          containing directory.

      --max-size <bytes>
          Match entries of at *most* <bytes> in *uncompressed* size.
      --min-size <bytes>
          Match entries of at *least* <bytes> in *uncompressed* size.

          Directory entries are 0 bytes in size, and symlink entries are the
          size required to store their target.

          TODO: Abbrevations such as 1k, 1M are not currently supported; the
          precise byte number must be provided, parseable as a u64.

  -m, --match[=<comp-sel>][:<pat-sel>] <pattern>
          Return true for entries whose name matches <pattern>.

          See section on "Selector syntax" for <comp-sel> and <pat-sel> for how
          the string argument <pattern> is interpreted into a string matching
          predicate against the entry name.
"#,
            Self::DEFLATE64_HELP_LINE,
            Self::BZIP2_HELP_LINE,
            Self::ZSTD_HELP_LINE,
            Self::LZMA_HELP_LINE,
            Self::XZ_HELP_LINE,
        )
    }

    pub fn generate_pattern_selector_help_text(ctx: PatSelContext) -> String {
        format!(
            r#"
## Selector syntax:

The string matching operations of {} expose an interface to
configure various pattern matching techniques on various components of the entry
name string.

{}

The entire range of search options is described below:

### Component selector (comp-sel):
comp-sel = path		[DEFAULT] (match full entry)
         = basename	(match only the final component of entry)
         = dirname	(match all except final component of entry)
         = ext		(match only the file extension, if available)

### Pattern selector (pat-sel):
{}
{}
Also note that glob and regex patterns require building this binary with the
"glob" and "rx" cargo features respectively. Specifying ':glob' or ':rx' without
the requisite feature support will produce an error. If the requisite feature is
not provided, the default is to use literal matching, which is supported in
all cases.

#### Pattern modifiers (pat-mod):
pat-mod  = :i	(use case-insensitive matching for the given pattern)
{}         = :p   (perform left-anchored "prefix" searches)
         = :s   (perform right-anchored "suffix" searches)

Pattern modifiers from (pat-mod) can be sequenced, e.g. ':i:p'. If ':p' and ':s'
are provided together, the result is to perform a doubly-anchored match, against
the entire string. For regexp matching with ':rx', ':p' and ':s' are converted
to '^' or '$' anchors in the regexp pattern string. If the pattern string also
contains '^' or '$' as well, no error is produced.

*Note:* not all pattern modifiers apply everywhere. In particular, {}':p' and ':s' are
incompatible with glob search and will produce an error.
"#,
            match ctx {
                PatSelContext::MatchOnly => "--match",
                PatSelContext::MatchAndTransform => "--match and --transform",
            },
            PatternSelectorType::generate_match_default_help_text(),
            PatternSelectorType::generate_pat_sel_help_section(ctx),
            PatternSelectorType::generate_glob_replacement_note(ctx),
            match ctx {
                PatSelContext::MatchOnly => "",
                PatSelContext::MatchAndTransform =>
                    "         = :g	(use multi-match behavior for string replacements)\n",
            },
            match ctx {
                PatSelContext::MatchOnly => "",
                PatSelContext::MatchAndTransform =>
                    "':g' only
applies to string replacement, and using it for a match expression like
'--match:rx:g' will produce an error. Additionally, ",
            },
        )
    }

    pub const INPUT_HELP_TEXT: &'static str = r#"
# Input arguments:
Zip file inputs to extract from can be specified by streaming from stdin, or as
at least one path pointing to an existing zip file.  Input arguments are always
specified after all output flags and entry specs on the command line. If no
positional argument is provided and --stdin is not present, an error will
be produced.

      --stdin
          If this argument is provided, the streaming API will be used to read
          entries as they are encountered, instead of filtering them beforehand
          as is done with file inputs. This disables some optimizations, but
          also avoids waiting for the entire input to buffer to start writing
          output, so can be used in a streaming context.

Positional paths:
  ZIP-PATH...
          Apply the entry specs to filter and rename entries to extract from all
          of the provided zip files. At least one zip path must be provided, and
          all provided paths must exist and point to an existing zip file. Pipes
          are not supported and will produce an error.

          If --stdin is provided, it will be read in a streaming manner before
          reading entries from any positional zip paths.
"#;
}

impl CommandFormat for Extract {
    const COMMAND_NAME: &'static str = "extract";
    const COMMAND_TABS: &'static str = "\t";
    const COMMAND_DESCRIPTION: &'static str =
        "Decompress and transform matching entries into a stream or directory.";

    const USAGE_LINE: &'static str =
        "[-h|--help] [OUTPUT-SPEC]... [ENTRY-SPEC]... [--stdin] [--] [ZIP-PATH]...";

    fn generate_help() -> String {
        format!(
            r#"
  -h, --help	Print help

# Output flags:
Where and how to collate the extracted entries.

## Directory extraction:
Extract entries into relative paths of a named directory according to the
entry's name.

  -d, --output-directory[:mkdir] <dir>
          Output directory path to write extracted entries into.
          Paths for extracted entries will be constructed by interpreting entry
          names as relative paths to the provided directory.

          If the provided path is not a directory, an error is produced. If the
          provided path does not exist, an error is produced, unless :mkdir is
          specified, which attempts to create the specified directory along with
          any missing parent directories.

          If not provided, entries will be extracted into the current directory
          (as if '-d .' had been provided).

## Pipe decompression:
Concatenate decompressed entry data into a pipe or file. Entry names are
effectively ignored. This disables some optimizations that are possible when
extracting to the filesystem.

      --stdout
          Concatenate all extracted entries and write them in order to stdout
          instead of writing anything to the filesystem.
          This will write output to stdout even if stdout is a tty.

  -f, --output-file[:append] <file>
          Write all entries into the specified file path <file>.

          The output file will be truncated if it already exists, unless :append
          is provided. If the specified file path could not be created
          (e.g. because the containing directory does not exist, or because the
          path exists but does not point to a regular file), an error
          is produced.

## Output teeing:
Entries may be *received* by one or more named outputs. Without any output names specified, the
above flags will produce a single receiver named "default". This is the default receiver used for
the -x/--extract argument unless otherwise specified. However, multiple named receivers may be
specified in sequence, separated by the --name flag:

      --name <name>
          Assign the output receiver created from the following output flags to the name <name>.

Note that the first output in a list need not have a name, as it will be assigned to the name
"default" if not provided.

'--stdout'	Creates a single default receiver decompressing contents to stdout.
'-d ./a'	Creates a single default receiver extracting entries into './a'.

'--name one -d ./a'
    Creates a single named receiver "one" extracting into './a'. -x/--extract
    must specify the name "one", or an error will be produced.
'--output-directory:mkdir ./a --name two --stdout'
    Creates a default receiver extracting into './a', which will be created if
    it does not exist, and a named receiver "two" concatenating into stdout.
'--name one -d ./a --name two -f ./b'
    Creates a named receiver "one" extracting into './a', and a second named receiver "two"
    concatenating into the file './b'.

# Entry specs:

After output flags are provided, entry specs are processed in order until an
input argument is reached. Entry specs are modelled after the arguments to
find(1), although "actions" are separated from "matching" expressions with
test clauses instead of being fully recursive like find(1).

The full specification of an entry spec is provided below
(we will use lowercase names to describe this grammar):

    entry-spec = [--expr match-expr --expr] [name-transform]... content-transform

1. (match-expr) matches against entries,
2. (name-transform) may transform the entry name string,
3. (content-transform) processes the entry content and writes it
                       to the output.

Note that only the "content transform" is required: each entry spec must
conclude with exactly one content transform, but the other arguments may
be omitted and will be set to their default values.

If no entry specs are provided, by default all entries are decompressed and written to the
output collator without modification. This behavior can be requested explicitly
with the command line:

    --expr -true --expr --identity --extract

*Note:* if a match-expr is provided, it *must* be surrounded with --expr arguments on both sides!
This is a necessary constraint of the current command line parsing.

{}

## Name transforms (name-transform):

Name transforms modify the entry name before writing the entry to the
output. Unlike match expressions, name transforms do not involve any boolean
logic, and instead are composed linearly, each processing the string produced by
the prior name transform in the series.

*Note:* name transforms do *not* perform any filtering, so if a string
replacement operation "fails", the entry name is simply returned unchanged.

Trivial:
      --identity	Return the entry name string unchanged.

If no name transforms are provided, it defaults to the behavior of --identity.

Basic:
These transformers do not perform any complex pattern matching, and instead add
or remove a fixed string from the entry name:

      --strip-components <num>
          Remove at most <num> directory components from the entry name.
          If <num> is greater than or equal the number of components in the
          entry dirname, then the basename of the entry is returned.
      --add-prefix <prefix>
          Prefix the entry name with a directory path <prefix>.
          A single separator '/' will be added after <prefix> before the rest of
          the entry name, and any trailing '/' in <prefix> will be trimmed
          before joining.

Complex:
These transformers perform complex pattern matching and replacement upon the
entry name string:

      --transform[=<comp-sel>][:<pat-sel>] <pattern> <replacement-spec>
          Extract the portion of the entry name corresponding to <comp-sel>,
          search it against <pattern> corresponding to <pat-sel>, and then
          replace the result with <replacement-spec>.

          If <pat-sel> == 'rx', then <replacement-spec> may contain references
          to numbered capture groups specified by <pattern>. Otherwise,
          <replacement-spec> is interpreted as a literal string.


## Content transforms (content-transform):

Content transforms determine how to interpret the content of the zip
entry itself.

*Note:* when multiple entry specs are provided on the command line, a single
entry may be matched more than once. In this case, the entry's content will be
teed to all the specified outputs.

  -x, --extract[=<name>]
          Decompress the entry's contents (if necessary) before writing it to
          the named output <name>, or the default output if the receiver name is
          not specified.

Attempting to extract an entry using an unsupported compression method with
-x/--extract will produce an error. In this case, --compression-method can be
used to filter out such entries.

{}
{}"#,
            Self::generate_match_expr_help_text(),
            Self::generate_pattern_selector_help_text(PatSelContext::MatchAndTransform),
            Self::INPUT_HELP_TEXT,
        )
    }

    fn parse_argv(mut argv: VecDeque<OsString>) -> Result<Self, ArgParseError> {
        let mut args: Vec<ExtractArg> = Vec::new();
        let mut stdin_flag: bool = false;
        let mut positional_zips: Vec<PathBuf> = Vec::new();

        let output_specs = OutputSpecs::parse_argv(&mut argv)?;

        while let Some(arg) = argv.pop_front() {
            match arg.as_encoded_bytes() {
                b"-h" | b"--help" => {
                    let help_text = Self::generate_full_help_text();
                    return Err(ArgParseError::StdoutMessage(help_text));
                }

                /* Transition to entry specs */
                /* Try content transforms first, as they are unambiguous sentinel values. */
                b"-x" | b"--extract" => {
                    args.push(ExtractArg::ContentTransform(ContentTransform::Extract {
                        name: None,
                    }));
                }
                arg_bytes if arg_bytes.starts_with(b"--extract=") => {
                    let name = arg
                        .into_string()
                        .map_err(|arg| {
                            Self::exit_arg_invalid(&format!(
                                "invalid unicode provided to --extract=<name>: {arg:?}"
                            ))
                        })?
                        .strip_prefix("--extract=")
                        .unwrap()
                        .to_string();
                    args.push(ExtractArg::ContentTransform(ContentTransform::Extract {
                        name: Some(name),
                    }));
                }

                /* Try name transforms next, as they only stack linearly and do not require CFG
                 * parsing of paired delimiters. */
                /* FIXME: none of these name transforms have any effect if --stdout is
                 * provided. Should we error or warn about this? */
                b"--identity" => {
                    args.push(ExtractArg::NameTransform(NameTransform::Trivial(
                        TrivialTransform::Identity,
                    )));
                }
                b"--strip-components" => {
                    let num: u8 = argv
                        .pop_front()
                        .ok_or_else(|| {
                            Self::exit_arg_invalid("no argument provided for --strip-component")
                        })?
                        .into_string()
                        .map_err(|num| {
                            Self::exit_arg_invalid(&format!(
                                "invalid unicode provided for --strip-component: {num:?}"
                            ))
                        })?
                        .parse::<u8>()
                        .map_err(|e| {
                            Self::exit_arg_invalid(&format!(
                                "failed to parse --strip-component arg {e:?} as u8"
                            ))
                        })?;
                    args.push(ExtractArg::NameTransform(NameTransform::Basic(
                        BasicTransform::StripComponents(num),
                    )));
                }
                b"--add-prefix" => {
                    let prefix = argv
                        .pop_front()
                        .ok_or_else(|| {
                            Self::exit_arg_invalid("no argument provided for --add-prefix")
                        })?
                        .into_string()
                        .map_err(|prefix| {
                            Self::exit_arg_invalid(&format!(
                                "invalid unicode provided for --add-prefix: {prefix:?}"
                            ))
                        })?;
                    args.push(ExtractArg::NameTransform(NameTransform::Basic(
                        BasicTransform::AddPrefix(prefix),
                    )));
                }
                arg_bytes if arg_bytes.starts_with(b"--transform") => {
                    let (comp_sel, pat_sel) =
                        parse_comp_and_pat_sel(arg_bytes, PatternContext::Replacement).ok_or_else(
                            || {
                                Self::exit_arg_invalid(&format!(
                                    "invalid --transform argument modifiers: {arg:?}"
                                ))
                            },
                        )?;
                    let pattern = argv
                        .pop_front()
                        .ok_or_else(|| {
                            Self::exit_arg_invalid("no <pattern> argument provided for --transform")
                        })?
                        .into_string()
                        .map_err(|pattern| {
                            Self::exit_arg_invalid(&format!(
                                "invalid unicode provided for --transform <pattern>: {pattern:?}"
                            ))
                        })?;
                    let replacement_spec = argv
                            .pop_front()
                            .ok_or_else(|| {
                                Self::exit_arg_invalid(
                                    "no <replacement-spec> argument provided for --transform",
                                )
                            })?
                            .into_string()
                            .map_err(|replacement_spec| {
                                Self::exit_arg_invalid(&format!(
                                    "invalid unicode provided for --transform <replacement-spec>: {replacement_spec:?}"
                                ))
                            })?;
                    args.push(ExtractArg::NameTransform(NameTransform::Complex(
                        ComplexTransform::Transform(TransformArg {
                            comp_sel,
                            pat_sel,
                            pattern,
                            replacement_spec,
                        }),
                    )));
                }

                /* Try parsing match specs! */
                b"--expr" => {
                    let match_expr = MatchExpression::parse_argv::<Self>(&mut argv)?;
                    args.push(ExtractArg::Match(match_expr));
                }

                /* Transition to input args */
                b"--stdin" => {
                    stdin_flag = true;
                }
                b"--" => break,
                arg_bytes => {
                    if arg_bytes.starts_with(b"-") {
                        return Err(Self::exit_arg_invalid(&format!(
                            "unrecognized flag {arg:?}"
                        )));
                    } else {
                        argv.push_front(arg);
                        break;
                    }
                }
            }
        }

        positional_zips.extend(argv.into_iter().map(|arg| arg.into()));
        if !stdin_flag && positional_zips.is_empty() {
            return Err(Self::exit_arg_invalid(
                "no zip input files were provided, and --stdin was not provided",
            ));
        };
        let input_spec = InputSpec {
            stdin_stream: stdin_flag,
            zip_paths: positional_zips,
        };

        let entry_specs = EntrySpec::parse_extract_args(args)?;

        Ok(Self {
            output_specs,
            entry_specs,
            input_spec,
        })
    }
}

impl crate::driver::ExecuteCommand for Extract {
    fn execute(self, err: impl std::io::Write) -> Result<(), crate::CommandError> {
        crate::extract::execute_extract(err, self)
    }
}
