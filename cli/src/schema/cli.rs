#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SectionName(String);

impl SectionName {
    pub fn create(name: impl Into<String>) -> Self {
        let name: String = name.into();
        assert!(!name.is_empty());
        assert!(name.chars().all(|c| c.is_ascii_uppercase()));
        Self(name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MetaVarName(String);

impl MetaVarName {
    pub fn create(name: impl Into<String>) -> Self {
        let name: String = name.into();
        assert!(!name.is_empty());
        assert!(name.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
        Self(name)
    }
}

pub trait MetaVar {
    fn choices(&self) -> Option<Vec<String>>;
}

pub enum FormatCaseElement {
    FormatRef(MetaVarName),
    Literal(String),
}

pub struct FormatCase {
    pub elements: Vec<FormatCaseElement>,
    pub description: Option<String>,
}

pub enum MetaVarKind {
    /* e.g. <num> */
    NameOnly(String),
    Format { cases: Vec<FormatCase> },
}

pub struct MetaVarDecl {
    pub id: MetaVarName,
    pub spec: MetaVarKind,
}

pub struct FlagSuffixCase {
    pub prefix_marker: &'static str,
    pub format: MetaVarName,
}

pub struct Flag {
    pub short: Option<char>,
    pub long: String,
    pub suffix_cases: Vec<FlagSuffixCase>,
    pub value: Option<MetaVarName>,
}

pub enum FlagCaseElement {
    SectionRef(SectionName),
    Literal(Flag),
    Optional(Box<FlagCaseElement>),
}

pub struct FlagCase {
    pub elements: Vec<FlagCaseElement>,
}

pub struct FlagsSectionDecl {
    pub id: SectionName,
    pub cases: Vec<FlagCase>,
}
