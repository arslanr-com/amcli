//! How a caller names a concept, and what happens when the name is not unique.
//!
//! The resolution ladder never guesses. Zero matches and several matches are
//! different answers with different remedies, and both carry enough information
//! to retry without a second exploratory round trip — that is what turns a
//! failed call into a successful one.

use std::collections::HashSet;

use amcli_model::{ConceptId, ConceptKind, ElementType, Layer, Model, RelType};

use crate::Graph;

/// A way of addressing concepts.
#[derive(Clone, Debug)]
pub enum Selector {
    /// `id:id-abc…` — the id as stored, the hex without Archi's `id-`, or a
    /// unique prefix of either. See [`by_id_or_prefix`].
    Id(String),
    /// `ApplicationComponent:Payment API` — name qualified by type.
    Typed { type_name: String, name: String },
    /// A bare name.
    Name(String),
    /// A name containing `*` or `?`.
    Glob(String),
    /// A filter expression.
    Filter(Expr),
}

/// What resolving a selector produced.
#[derive(Clone, Debug)]
pub enum Resolution {
    One(ConceptId),
    /// Several concepts matched where one was needed. Each candidate carries a
    /// selector string the caller can paste back verbatim.
    Ambiguous(Vec<ConceptId>),
    /// Nothing matched, with the nearest names we could find.
    NotFound {
        suggestions: Vec<ConceptId>,
    },
}

impl Selector {
    pub fn parse(s: &str) -> Selector {
        if let Some(id) = s.strip_prefix("id:") {
            return Selector::Id(id.to_string());
        }
        // A filter expression is anything carrying one of the operators. Testing
        // for the operator rather than for a keyword keeps `name=X` working
        // without quoting gymnastics.
        if looks_like_filter(s)
            && let Ok(e) = Expr::parse(s)
        {
            return Selector::Filter(e);
        }
        if s.contains('*') || s.contains('?') {
            return Selector::Glob(s.to_string());
        }
        if let Some((ty, name)) = s.split_once(':')
            && (ElementType::from_str(ty).is_some() || RelType::from_str(ty).is_some())
        {
            return Selector::Typed {
                type_name: ty.to_string(),
                name: name.trim_matches('"').to_string(),
            };
        }
        Selector::Name(s.to_string())
    }

    /// Every concept this selector matches, in a stable order.
    pub fn matches(&self, g: &Graph<'_>) -> Vec<ConceptId> {
        let m = g.model();
        let mut out: Vec<ConceptId> = match self {
            Selector::Id(id) => {
                by_id_or_prefix(m.concepts_with_ids().map(|(i, c)| (i, c.id.as_str())), id)
            }
            Selector::Name(name) => {
                // Case-sensitive first: an exact spelling should win outright
                // over a differently-cased twin.
                let all = g.by_name(name);
                let exact: Vec<ConceptId> =
                    all.iter().copied().filter(|c| m.concept(*c).name == *name).collect();
                if exact.is_empty() { all.to_vec() } else { exact }
            }
            Selector::Typed { type_name, name } => g
                .by_name(name)
                .iter()
                .copied()
                .filter(|c| type_matches(&m.concept(*c).kind, type_name))
                .collect(),
            Selector::Glob(pat) => m
                .concepts_with_ids()
                .filter(|(_, c)| glob_match(pat, &c.name))
                .map(|(i, _)| i)
                .collect(),
            Selector::Filter(e) => {
                m.concepts_with_ids().map(|(i, _)| i).filter(|c| e.eval(g, *c)).collect()
            }
        };
        out.sort_by_key(|c| {
            let concept = m.concept(*c);
            (concept.kind.name().to_string(), concept.name.clone(), *c)
        });
        out.dedup();
        out
    }

    /// Resolve to exactly one concept, reporting *why* not when that fails.
    pub fn resolve_one(&self, g: &Graph<'_>) -> Resolution {
        let found = self.matches(g);
        match found.len() {
            1 => Resolution::One(found[0]),
            0 => Resolution::NotFound { suggestions: self.suggest(g) },
            _ => Resolution::Ambiguous(found),
        }
    }

    /// Nearest names, so a miss comes back with something to try instead of
    /// forcing the caller into another search.
    fn suggest(&self, g: &Graph<'_>) -> Vec<ConceptId> {
        let needle = match self {
            Selector::Name(s) | Selector::Glob(s) => s.to_lowercase(),
            Selector::Typed { name, .. } => name.to_lowercase(),
            // Names near an id are noise, not help.
            Selector::Id(_) | Selector::Filter(_) => return Vec::new(),
        };
        if needle.is_empty() {
            return Vec::new();
        }
        let m = g.model();
        let mut scored: Vec<(usize, ConceptId)> = m
            .concepts_with_ids()
            .filter_map(|(i, c)| {
                let name = c.name.to_lowercase();
                if name.is_empty() {
                    return None;
                }
                if name.contains(&needle) || needle.contains(&name) {
                    return Some((0, i));
                }
                let d = edit_distance(&needle, &name);
                (d <= needle.len().div_ceil(2)).then_some((d, i))
            })
            .collect();
        scored.sort_by_key(|(d, c)| (*d, m.concept(*c).name.clone(), *c));
        scored.truncate(5);
        scored.into_iter().map(|(_, c)| c).collect()
    }
}

/// The shortest prefix of an id's hex that `id:` accepts.
///
/// Four characters is 65,536 values: enough that a prefix an agent copied off a
/// row is one thing on any real model, and short enough that a clash is
/// reported as ambiguous with the candidates listed rather than as a wall.
pub const ID_PREFIX_MIN: usize = 4;

/// Everything `sel` names among `ids`, which are `(handle, id)` pairs.
///
/// Archi writes ids as `id-` and thirty-two hex characters; older models
/// carry eight bare hex characters. A reader who copies the hex off a row
/// without the `id-`, or the first eight characters of it, meant the same
/// thing as the whole string, so all three resolve: the id exactly as stored
/// wins outright; then the hex compared without the prefix on either side;
/// then, at [`ID_PREFIX_MIN`] characters or more, a prefix of that hex. A
/// prefix two ids share comes back as both, and the caller reports the
/// ambiguity the way it reports two elements sharing a name.
pub fn by_id_or_prefix<'a, T: Copy>(ids: impl Iterator<Item = (T, &'a str)>, sel: &str) -> Vec<T> {
    fn bare(id: &str) -> String {
        id.strip_prefix("id-").unwrap_or(id).to_ascii_lowercase()
    }
    let want = bare(sel);
    if want.is_empty() {
        return Vec::new();
    }
    let (mut exact, mut same_hex, mut prefixed) = (Vec::new(), Vec::new(), Vec::new());
    for (handle, id) in ids {
        if id == sel {
            exact.push(handle);
            continue;
        }
        let hex = bare(id);
        if hex == want {
            same_hex.push(handle);
        } else if want.len() >= ID_PREFIX_MIN && hex.starts_with(&want) {
            prefixed.push(handle);
        }
    }
    if !exact.is_empty() {
        exact
    } else if !same_hex.is_empty() {
        same_hex
    } else {
        prefixed
    }
}

fn type_matches(kind: &ConceptKind, want: &str) -> bool {
    kind.name().eq_ignore_ascii_case(want)
        || matches!(kind, ConceptKind::Relationship(r) if RelType::from_str(want) == Some(*r))
        || matches!(kind, ConceptKind::Element(e) if ElementType::from_str(want) == Some(*e))
}

fn looks_like_filter(s: &str) -> bool {
    ["=", "~", "^=", ">", "<"].iter().any(|op| s.contains(op))
        && !s.starts_with('*')
        && s.split_whitespace().next().is_some_and(|w| {
            w.contains('=') || w.contains('~') || w.contains('>') || w.contains('<')
        })
}

// ---- filter expressions ---------------------------------------------------

#[derive(Clone, Debug)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Term { key: Key, op: Op, value: String, regex: Option<regex::Regex> },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Key {
    Id,
    Name,
    Type,
    /// `element` or `relation`. A query for "everything in the Application
    /// layer" otherwise comes back with relationships mixed in, and those
    /// render as rows with an empty name.
    Kind,
    Layer,
    Folder,
    Doc,
    Degree,
    Prop(String),
    InRel(String),
    OutRel(String),
    View,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Eq,
    Contains,
    Prefix,
    Regex,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ParseError(pub String);

/// The filter vocabulary, in one place so the parser's complaint and the CLI's
/// hint cannot drift apart.
pub const FIELDS: &str =
    "id name type kind layer folder doc deg view prop:KEY in:RelType out:RelType";

impl Expr {
    pub fn parse(s: &str) -> Result<Expr, ParseError> {
        let tokens = tokenize(s);
        let mut p = Parser { tokens, pos: 0 };
        let e = p.parse_or()?;
        if p.pos < p.tokens.len() {
            return Err(ParseError(format!("unexpected `{}`", p.tokens[p.pos])));
        }
        Ok(e)
    }

    pub fn eval(&self, g: &Graph<'_>, c: ConceptId) -> bool {
        match self {
            Expr::And(a, b) => a.eval(g, c) && b.eval(g, c),
            Expr::Or(a, b) => a.eval(g, c) || b.eval(g, c),
            Expr::Not(a) => !a.eval(g, c),
            Expr::Term { key, op, value, regex } => {
                eval_term(g, c, key, *op, value, regex.as_ref())
            }
        }
    }
}

fn eval_term(
    g: &Graph<'_>,
    c: ConceptId,
    key: &Key,
    op: Op,
    value: &str,
    re: Option<&regex::Regex>,
) -> bool {
    let m: &Model = g.model();
    let concept = m.concept(c);

    // Degree is the one purely numeric field, so it gets the comparison
    // operators and the string ones do not.
    if *key == Key::Degree {
        let (i, o) = g.degree(c);
        let Ok(want) = value.parse::<i64>() else { return false };
        return num_op(op, (i + o) as i64, want);
    }

    // Relationship-shaped keys ask about the neighbourhood, not this concept.
    match key {
        Key::InRel(rel) | Key::OutRel(rel) => {
            let dir = if matches!(key, Key::InRel(_)) { crate::Dir::In } else { crate::Dir::Out };
            let want_rel = RelType::from_str(rel);
            return g.neighbors(c, dir, &crate::EdgeFilter::default()).iter().any(|a| {
                let rel_ok = match (&m.concept(a.rel).kind, want_rel) {
                    (ConceptKind::Relationship(r), Some(w)) => *r == w,
                    _ => rel.is_empty(),
                };
                rel_ok && (value.is_empty() || str_op(op, &m.concept(a.other).name, value, re))
            });
        }
        Key::View => {
            let on = g.views_of(c);
            // A bare integer asks how many views, a string asks which. Without
            // the numeric reading there is no way to ask the one question a
            // "draw everything at least once" invariant depends on — which
            // concepts are on no view at all — and `view=0` silently matched
            // nothing instead of saying so.
            if let Ok(want) = value.parse::<i64>() {
                return num_op(op, on.len() as i64, want);
            }
            return on.iter().any(|v| value.is_empty() || str_op(op, &m.view(*v).name, value, re));
        }
        _ => {}
    }

    let haystack: String = match key {
        Key::Id => concept.id.clone(),
        Key::Name => concept.name.clone(),
        Key::Type => concept.kind.name().to_string(),
        Key::Kind => {
            if concept.kind.is_relationship() { "relation" } else { "element" }.to_string()
        }
        Key::Layer => concept.kind.layer().map(Layer::as_str).unwrap_or("").to_string(),
        Key::Folder => m.folder_path_of(concept).to_string(),
        Key::Doc => m.documentation(concept.node).unwrap_or_default(),
        Key::Prop(k) => {
            let props = m.properties(concept.node);
            match props.iter().find(|(pk, _)| pk.eq_ignore_ascii_case(k)) {
                Some((_, v)) => v.clone(),
                // A property that is absent matches only a negated test.
                None => return op == Op::Ne,
            }
        }
        Key::Degree | Key::InRel(_) | Key::OutRel(_) | Key::View => unreachable!(),
    };

    // `Triggering` and `TriggeringRelationship` name the same type. `relation
    // add Triggering` takes the first, so a query that only takes the second is
    // a second vocabulary to learn — and one that answers 0 rather than saying
    // so. Only the type field is translated: `name=Access` must keep meaning an
    // element called Access.
    let value = match key {
        Key::Type => canonical_type_name(value).unwrap_or(value),
        _ => value,
    };
    str_op(op, &haystack, value, re)
}

/// How a type is spelled in the file and in output, given any spelling a caller
/// might reasonably use.
///
/// `Triggering`, `TriggeringRelationship` and `archimate:TriggeringRelationship`
/// all answer `TriggeringRelationship`; an element type answers its own name.
/// `None` means the string is not a type this build knows.
pub fn canonical_type_name(s: &str) -> Option<&'static str> {
    if let Some(e) = ElementType::from_str(s) {
        return Some(e.info().short);
    }
    // The relationship's own name, as written in the file: `info().short` is
    // the bare `Triggering`, but every row amcli prints says
    // `TriggeringRelationship`, so that is what a comparison has to use.
    RelType::from_str(s).map(|r| r.info().xsi.trim_start_matches("archimate:"))
}

fn num_op(op: Op, have: i64, want: i64) -> bool {
    match op {
        Op::Eq => have == want,
        Op::Ne => have != want,
        Op::Gt => have > want,
        Op::Lt => have < want,
        Op::Ge => have >= want,
        Op::Le => have <= want,
        Op::Contains | Op::Prefix | Op::Regex => false,
    }
}

fn str_op(op: Op, haystack: &str, value: &str, re: Option<&regex::Regex>) -> bool {
    let h = haystack.to_lowercase();
    let v = value.to_lowercase();
    match op {
        Op::Eq => h == v,
        Op::Ne => h != v,
        Op::Contains => h.contains(&v),
        Op::Prefix => h.starts_with(&v),
        Op::Regex => re.is_some_and(|r| r.is_match(haystack)),
        // Comparisons only make sense on degree.
        Op::Gt | Op::Lt | Op::Ge | Op::Le => false,
    }
}

struct Parser {
    tokens: Vec<String>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(String::as_str)
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.peek().is_some_and(|t| t.eq_ignore_ascii_case("or")) {
            self.pos += 1;
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        while self.peek().is_some_and(|t| t.eq_ignore_ascii_case("and")) {
            self.pos += 1;
            let right = self.parse_unary()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if self.peek().is_some_and(|t| t.eq_ignore_ascii_case("not")) {
            self.pos += 1;
            return Ok(Expr::Not(Box::new(self.parse_unary()?)));
        }
        if self.peek() == Some("(") {
            self.pos += 1;
            let e = self.parse_or()?;
            if self.peek() != Some(")") {
                return Err(ParseError("missing `)`".into()));
            }
            self.pos += 1;
            return Ok(e);
        }
        self.parse_term()
    }

    fn parse_term(&mut self) -> Result<Expr, ParseError> {
        let tok = self
            .peek()
            .ok_or_else(|| {
                ParseError("expected a condition like `type=ApplicationComponent`".into())
            })?
            .to_string();
        self.pos += 1;

        let (raw_key, op, value) = split_term(&tok)
            .ok_or_else(|| ParseError(format!("`{tok}` is not a condition; expected key=value")))?;

        let key = match raw_key.split_once(':') {
            Some(("prop", k)) => Key::Prop(k.to_string()),
            Some(("in", r)) => Key::InRel(r.to_string()),
            Some(("out", r)) => Key::OutRel(r.to_string()),
            _ => match raw_key.to_lowercase().as_str() {
                "id" => Key::Id,
                "name" => Key::Name,
                "type" => Key::Type,
                "kind" => Key::Kind,
                "layer" => Key::Layer,
                "folder" => Key::Folder,
                "doc" | "documentation" => Key::Doc,
                "deg" | "degree" => Key::Degree,
                // Plural too, because that is what the output column is called.
                "view" | "views" => Key::View,
                other => {
                    return Err(ParseError(format!(
                        "unknown field `{other}`; try one of {FIELDS}"
                    )));
                }
            },
        };

        let value = value.trim_matches(|c| c == '"' || c == '\'').to_string();
        let regex = if op == Op::Regex {
            Some(
                regex::Regex::new(&value)
                    .map_err(|e| ParseError(format!("bad regex `{value}`: {e}")))?,
            )
        } else {
            None
        };
        Ok(Expr::Term { key, op, value, regex })
    }
}

fn split_term(tok: &str) -> Option<(&str, Op, &str)> {
    // Longest operators first so `>=` is not read as `>`.
    for (pat, op) in [
        ("^=", Op::Prefix),
        ("=~", Op::Regex),
        ("!=", Op::Ne),
        (">=", Op::Ge),
        ("<=", Op::Le),
        ("~", Op::Contains),
        ("=", Op::Eq),
        (">", Op::Gt),
        ("<", Op::Lt),
    ] {
        if let Some(i) = tok.find(pat) {
            return Some((&tok[..i], op, &tok[i + pat.len()..]));
        }
    }
    None
}

/// Split on whitespace, keeping quoted runs together so a value may contain
/// spaces.
///
/// Parentheses are the awkward part, because they mean grouping in a filter and
/// alternation in a regex — `name=~^P(ing|ong)$` must not be chopped into three
/// tokens. The rule: `(` groups only at the start of a token, and `)` closes a
/// group only when one is actually open. A regex therefore never opens a group,
/// so its closing paren stays where it belongs.
fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0usize;

    for ch in s.chars() {
        match ch {
            '"' | '\'' if quote.is_none() => {
                quote = Some(ch);
                cur.push(ch);
            }
            c if Some(c) == quote => {
                quote = None;
                cur.push(c);
            }
            _ if quote.is_some() => cur.push(ch),
            ' ' | '\t' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            '(' if cur.is_empty() => {
                depth += 1;
                out.push("(".to_string());
            }
            ')' if depth > 0 => {
                depth -= 1;
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(")".to_string());
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `*` matches any run, `?` matches one character. Case-insensitive.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, usize::MAX, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ti;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Levenshtein distance, used only to rank suggestions.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Element and relationship type names a caller may use in a `-t` flag.
pub fn known_type_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = ElementType::ALL.iter().map(|e| e.info().short).collect();
    v.extend(RelType::ALL.iter().map(|r| r.info().short));
    v
}

/// Does this model contain concepts of a type this build does not know?
///
/// A `-t` filter naming one of those is legitimate — the concepts are there and
/// searchable — so it must not be rejected along with the typos.
pub fn model_knows_type(m: &Model, name: &str) -> bool {
    unknown_type_names(m).iter().any(|t| t.eq_ignore_ascii_case(name))
}

/// Type names close enough to a rejected one to be worth suggesting.
pub fn similar_type_names(name: &str) -> Vec<&'static str> {
    let needle = name.to_lowercase();
    let mut scored: Vec<(usize, &'static str)> = known_type_names()
        .into_iter()
        .filter_map(|t| {
            let lower = t.to_lowercase();
            if lower.contains(&needle) || needle.contains(&lower) {
                return Some((0, t));
            }
            let d = edit_distance(&needle, &lower);
            (d <= needle.len().div_ceil(2)).then_some((d, t))
        })
        .collect();
    scored.sort_by_key(|(d, t)| (*d, *t));
    scored.truncate(5);
    scored.into_iter().map(|(_, t)| t).collect()
}

/// Types that are neither an element nor a relationship we know about.
pub fn unknown_type_names(m: &Model) -> HashSet<String> {
    m.concepts()
        .filter_map(|c| match &c.kind {
            ConceptKind::Unknown { xsi, .. } => Some(xsi.clone()),
            _ => None,
        })
        .collect()
}
