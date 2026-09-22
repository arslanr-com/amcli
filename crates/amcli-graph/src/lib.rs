//! Graph indices and queries.
//!
//! The index is derived and disposable: it is rebuilt from the model in one
//! pass and is never a source of truth. Holding a `Graph` borrows the `Model`,
//! so the compiler enforces the one rule that matters — you cannot mutate the
//! model while a stale index is still in hand.

use std::collections::{HashMap, HashSet, VecDeque};

use amcli_model::{ConceptId, ConceptKind, ElementType, Layer, Model, RelType, ViewId};

pub mod select;
pub use select::{Resolution, Selector};

/// One step out of a concept: the relationship traversed and where it lands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Arc {
    /// The relationship concept itself, so callers can name it and edit it.
    pub rel: ConceptId,
    pub other: ConceptId,
    pub dir: Dir,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Out,
    In,
    Both,
}

impl Dir {
    pub fn parse(s: &str) -> Option<Dir> {
        Some(match s {
            "out" => Dir::Out,
            "in" => Dir::In,
            "both" => Dir::Both,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Dir::Out => "out",
            Dir::In => "in",
            Dir::Both => "both",
        }
    }
}

/// Which edges a traversal may cross. Note this constrains *traversal*; a filter
/// on concept type is applied to the output instead, so that filtering by a leaf
/// type does not make multi-hop queries return nothing.
#[derive(Clone, Default, Debug)]
pub struct EdgeFilter {
    pub rels: Option<HashSet<RelType>>,
}

impl EdgeFilter {
    pub fn only(rels: impl IntoIterator<Item = RelType>) -> EdgeFilter {
        EdgeFilter { rels: Some(rels.into_iter().collect()) }
    }

    fn allows(&self, m: &Model, rel: ConceptId) -> bool {
        let Some(want) = &self.rels else { return true };
        match m.concept(rel).kind {
            ConceptKind::Relationship(r) => want.contains(&r),
            _ => false,
        }
    }
}

/// A node set plus every edge the model has between those nodes.
#[derive(Clone, Debug, Default)]
pub struct Subgraph {
    /// Concepts, each with the fewest hops it took to reach.
    pub nodes: Vec<(ConceptId, u32)>,
    /// Relationship concepts whose endpoints are both in `nodes`.
    pub edges: Vec<ConceptId>,
    /// True when a limit stopped the walk before it ran out of graph.
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub struct Path {
    /// Concepts from source to target inclusive.
    pub nodes: Vec<ConceptId>,
    /// The relationship crossed at each step; one shorter than `nodes`.
    pub edges: Vec<ConceptId>,
}

impl Path {
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }
}

pub struct Graph<'m> {
    model: &'m Model,
    out: Vec<Vec<Arc>>,
    inc: Vec<Vec<Arc>>,
    /// Lower-cased name to every concept bearing it. Multi-valued on purpose:
    /// duplicate names are common, and swallowing the ambiguity is what made
    /// the old tool report a name that existed twice as "not found".
    by_name: HashMap<String, Vec<ConceptId>>,
    /// Resolved endpoints, for relationships whose ends both exist.
    ends: Vec<Option<(ConceptId, ConceptId)>>,
    /// Relationships whose `source` or `target` names a missing id.
    dangling: Vec<ConceptId>,
    /// Which views each concept is drawn on, in document order.
    ///
    /// Built here, once, because the alternative is what the `view` filter used
    /// to do: re-walk every view's descendants for every candidate concept,
    /// which is quadratic and was slow enough to be worth avoiding on a real
    /// model. It is also what lets a concept row carry a view count at all.
    on_views: Vec<Vec<ViewId>>,
}

impl<'m> Graph<'m> {
    pub fn build(model: &'m Model) -> Graph<'m> {
        // Sized by slots, not by live concepts: a ConceptId is a stable handle
        // and deleted slots keep their index.
        let n = model.concept_slots();
        let mut g = Graph {
            model,
            out: vec![Vec::new(); n],
            inc: vec![Vec::new(); n],
            by_name: HashMap::new(),
            ends: vec![None; n],
            dangling: Vec::new(),
            on_views: vec![Vec::new(); n],
        };

        for (view, v) in model.views_with_ids() {
            for node in model.doc.descendants(v.node) {
                // A diagram object names its concept with one attribute or the
                // other depending on whether it is a box or a line.
                let referenced = model
                    .doc
                    .attr(node, "archimateElement")
                    .or_else(|| model.doc.attr(node, "archimateRelationship"));
                if let Some(id) = referenced
                    && let Some(c) = model.concept_by_id(&id)
                {
                    let seen = &mut g.on_views[c.0 as usize];
                    // A concept may appear on one view several times; the
                    // question callers ask is "which views", not "how many
                    // boxes".
                    if seen.last() != Some(&view) && !seen.contains(&view) {
                        seen.push(view);
                    }
                }
            }
        }

        for (id, c) in model.concepts_with_ids() {
            if !c.name.is_empty() {
                g.by_name.entry(c.name.to_lowercase()).or_default().push(id);
            }
            if !c.kind.is_relationship() {
                continue;
            }
            let ends = c
                .source
                .as_deref()
                .and_then(|s| model.concept_by_id(s))
                .zip(c.target.as_deref().and_then(|t| model.concept_by_id(t)));
            match ends {
                Some((s, t)) => {
                    g.ends[id.0 as usize] = Some((s, t));
                    g.out[s.0 as usize].push(Arc { rel: id, other: t, dir: Dir::Out });
                    g.inc[t.0 as usize].push(Arc { rel: id, other: s, dir: Dir::In });
                }
                None => g.dangling.push(id),
            }
        }

        // A box drawn inside another box stands for the relationship between
        // the two, and Archi draws no line for it. So a relationship whose
        // ends a view nests is on that view, line or no line — otherwise every
        // nested drawing reports its containments as drawn nowhere.
        for (view, _) in model.views_with_ids() {
            for (outer, inner) in model.view_nestings(view) {
                let (Some(o), Some(i)) = (model.concept_by_id(&outer), model.concept_by_id(&inner))
                else {
                    continue;
                };
                let between: Vec<ConceptId> = g.out[o.0 as usize]
                    .iter()
                    .filter(|a| a.other == i)
                    .chain(g.out[i.0 as usize].iter().filter(|a| a.other == o))
                    .map(|a| a.rel)
                    .collect();
                for rel in between {
                    let seen = &mut g.on_views[rel.0 as usize];
                    if !seen.contains(&view) {
                        seen.push(view);
                    }
                }
            }
        }
        g
    }

    pub fn model(&self) -> &'m Model {
        self.model
    }

    pub fn ends(&self, rel: ConceptId) -> Option<(ConceptId, ConceptId)> {
        self.ends[rel.0 as usize]
    }

    /// Relationships pointing at an id that is not in the model.
    pub fn dangling(&self) -> &[ConceptId] {
        &self.dangling
    }

    pub fn degree(&self, c: ConceptId) -> (usize, usize) {
        (self.inc[c.0 as usize].len(), self.out[c.0 as usize].len())
    }

    /// The views this concept is drawn on. Empty means it is in the model but
    /// on no diagram, which is the thing `view=0` asks about.
    pub fn views_of(&self, c: ConceptId) -> &[ViewId] {
        &self.on_views[c.0 as usize]
    }

    pub fn neighbors(&self, c: ConceptId, dir: Dir, f: &EdgeFilter) -> Vec<Arc> {
        let i = c.0 as usize;
        let mut v: Vec<Arc> = match dir {
            Dir::Out => self.out[i].clone(),
            Dir::In => self.inc[i].clone(),
            Dir::Both => self.out[i].iter().chain(self.inc[i].iter()).copied().collect(),
        };
        v.retain(|a| f.allows(self.model, a.rel));
        v
    }

    /// Concepts within `k` hops, plus **every** relationship between them.
    ///
    /// The second pass is what makes this an induced subgraph rather than a BFS
    /// tree: cross-edges and back-edges are part of the answer, so cycles are
    /// visible instead of silently pruned.
    pub fn k_hop(
        &self,
        seeds: &[ConceptId],
        k: u32,
        dir: Dir,
        f: &EdgeFilter,
        max_nodes: usize,
    ) -> Subgraph {
        let mut depth: HashMap<ConceptId, u32> = HashMap::new();
        let mut queue: VecDeque<(ConceptId, u32)> = VecDeque::new();
        let mut truncated = false;

        for &s in seeds {
            if depth.insert(s, 0).is_none() {
                queue.push_back((s, 0));
            }
        }
        while let Some((cur, d)) = queue.pop_front() {
            if d >= k {
                continue;
            }
            for a in self.neighbors(cur, dir, f) {
                if depth.contains_key(&a.other) {
                    continue;
                }
                if depth.len() >= max_nodes {
                    truncated = true;
                    break;
                }
                depth.insert(a.other, d + 1);
                queue.push_back((a.other, d + 1));
            }
            if truncated {
                break;
            }
        }

        let mut nodes: Vec<(ConceptId, u32)> = depth.iter().map(|(c, d)| (*c, *d)).collect();
        nodes.sort_by_key(|(c, d)| (*d, self.model.concept(*c).name.clone(), *c));

        let inside: HashSet<ConceptId> = depth.keys().copied().collect();
        let mut edges: Vec<ConceptId> = Vec::new();
        for &(c, _) in &nodes {
            for a in &self.out[c.0 as usize] {
                if inside.contains(&a.other) && f.allows(self.model, a.rel) {
                    edges.push(a.rel);
                }
            }
        }
        edges.sort();
        edges.dedup();

        Subgraph { nodes, edges, truncated }
    }

    /// Fewest hops from `a` to `b`, or `None` if they are not connected.
    pub fn shortest_path(
        &self,
        a: ConceptId,
        b: ConceptId,
        dir: Dir,
        f: &EdgeFilter,
    ) -> Option<Path> {
        if a == b {
            return Some(Path { nodes: vec![a], edges: Vec::new() });
        }
        let mut prev: HashMap<ConceptId, (ConceptId, ConceptId)> = HashMap::new();
        let mut seen: HashSet<ConceptId> = HashSet::from([a]);
        let mut queue = VecDeque::from([a]);

        while let Some(cur) = queue.pop_front() {
            for arc in self.neighbors(cur, dir, f) {
                if !seen.insert(arc.other) {
                    continue;
                }
                prev.insert(arc.other, (cur, arc.rel));
                if arc.other == b {
                    return Some(self.rebuild(a, b, &prev));
                }
                queue.push_back(arc.other);
            }
        }
        None
    }

    fn rebuild(
        &self,
        a: ConceptId,
        b: ConceptId,
        prev: &HashMap<ConceptId, (ConceptId, ConceptId)>,
    ) -> Path {
        let mut nodes = vec![b];
        let mut edges = Vec::new();
        let mut cur = b;
        while cur != a {
            let (p, rel) = prev[&cur];
            edges.push(rel);
            nodes.push(p);
            cur = p;
        }
        nodes.reverse();
        edges.reverse();
        Path { nodes, edges }
    }

    /// Every simple path up to `max_len` hops, capped at `max_paths`.
    pub fn all_paths(
        &self,
        a: ConceptId,
        b: ConceptId,
        max_len: u32,
        max_paths: usize,
        dir: Dir,
        f: &EdgeFilter,
    ) -> (Vec<Path>, bool) {
        let mut found = Vec::new();
        let mut stack = vec![a];
        let mut edges = Vec::new();
        let mut on_path: HashSet<ConceptId> = HashSet::from([a]);
        let mut truncated = false;
        self.walk(
            a,
            b,
            max_len,
            max_paths,
            dir,
            f,
            &mut stack,
            &mut edges,
            &mut on_path,
            &mut found,
            &mut truncated,
        );
        (found, truncated)
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(
        &self,
        cur: ConceptId,
        target: ConceptId,
        max_len: u32,
        max_paths: usize,
        dir: Dir,
        f: &EdgeFilter,
        nodes: &mut Vec<ConceptId>,
        edges: &mut Vec<ConceptId>,
        on_path: &mut HashSet<ConceptId>,
        found: &mut Vec<Path>,
        truncated: &mut bool,
    ) {
        if found.len() >= max_paths {
            *truncated = true;
            return;
        }
        if cur == target && nodes.len() > 1 {
            found.push(Path { nodes: nodes.clone(), edges: edges.clone() });
            return;
        }
        if edges.len() as u32 >= max_len {
            return;
        }
        for arc in self.neighbors(cur, dir, f) {
            if on_path.contains(&arc.other) {
                continue;
            }
            nodes.push(arc.other);
            edges.push(arc.rel);
            on_path.insert(arc.other);
            self.walk(
                arc.other, target, max_len, max_paths, dir, f, nodes, edges, on_path, found,
                truncated,
            );
            on_path.remove(&arc.other);
            edges.pop();
            nodes.pop();
        }
    }

    /// Containment: what this concept is part of, following the structural
    /// relationships that actually express whole-part.
    pub fn ancestors(&self, c: ConceptId, via: &[RelType]) -> Vec<ConceptId> {
        self.transitive(c, Dir::In, via)
    }

    /// Containment the other way: what this concept is made of.
    pub fn descendants(&self, c: ConceptId, via: &[RelType]) -> Vec<ConceptId> {
        self.transitive(c, Dir::Out, via)
    }

    fn transitive(&self, c: ConceptId, dir: Dir, via: &[RelType]) -> Vec<ConceptId> {
        let f = EdgeFilter::only(via.iter().copied());
        let mut seen = HashSet::from([c]);
        let mut queue = VecDeque::from([c]);
        let mut out = Vec::new();
        while let Some(cur) = queue.pop_front() {
            for arc in self.neighbors(cur, dir, &f) {
                if seen.insert(arc.other) {
                    out.push(arc.other);
                    queue.push_back(arc.other);
                }
            }
        }
        out
    }

    /// The default notion of containment: composition and aggregation.
    pub const CONTAINMENT: [RelType; 2] = [RelType::Composition, RelType::Aggregation];

    /// Everything reachable from the seeds, with the hop count and the
    /// relationship that first brought each concept in — the "why" is what makes
    /// an impact report actionable rather than just a list.
    pub fn impact(
        &self,
        seeds: &[ConceptId],
        dir: Dir,
        max_depth: Option<u32>,
        f: &EdgeFilter,
        max_nodes: usize,
    ) -> (Vec<(ConceptId, u32, Option<ConceptId>)>, bool) {
        let mut seen: HashMap<ConceptId, (u32, Option<ConceptId>)> = HashMap::new();
        let mut queue = VecDeque::new();
        let mut truncated = false;
        for &s in seeds {
            seen.insert(s, (0, None));
            queue.push_back(s);
        }
        while let Some(cur) = queue.pop_front() {
            let d = seen[&cur].0;
            if max_depth.is_some_and(|m| d >= m) {
                continue;
            }
            for arc in self.neighbors(cur, dir, f) {
                if seen.contains_key(&arc.other) {
                    continue;
                }
                if seen.len() >= max_nodes {
                    truncated = true;
                    break;
                }
                seen.insert(arc.other, (d + 1, Some(arc.rel)));
                queue.push_back(arc.other);
            }
            if truncated {
                break;
            }
        }
        let mut out: Vec<(ConceptId, u32, Option<ConceptId>)> = seen
            .into_iter()
            .filter(|(c, _)| !seeds.contains(c))
            .map(|(c, (d, r))| (c, d, r))
            .collect();
        out.sort_by_key(|(c, d, _)| (*d, self.model.concept(*c).name.clone(), *c));
        (out, truncated)
    }

    /// Strongly connected components with more than one member, plus self-loops:
    /// Tarjan, iterative so a deep model cannot blow the stack.
    pub fn cycles(&self, f: &EdgeFilter) -> Vec<Vec<ConceptId>> {
        let n = self.out.len();
        let mut index = vec![u32::MAX; n];
        let mut low = vec![0u32; n];
        let mut on_stack = vec![false; n];
        let mut stack: Vec<ConceptId> = Vec::new();
        let mut next = 0u32;
        let mut result = Vec::new();

        for start in 0..n {
            if index[start] != u32::MAX {
                continue;
            }
            // (node, position in its neighbour list)
            let mut call: Vec<(usize, usize)> = vec![(start, 0)];
            index[start] = next;
            low[start] = next;
            next += 1;
            stack.push(ConceptId(start as u32));
            on_stack[start] = true;

            while let Some((v, pos)) = call.pop() {
                let arcs: Vec<Arc> =
                    self.out[v].iter().copied().filter(|a| f.allows(self.model, a.rel)).collect();
                if pos < arcs.len() {
                    call.push((v, pos + 1));
                    let w = arcs[pos].other.0 as usize;
                    if index[w] == u32::MAX {
                        index[w] = next;
                        low[w] = next;
                        next += 1;
                        stack.push(ConceptId(w as u32));
                        on_stack[w] = true;
                        call.push((w, 0));
                    } else if on_stack[w] {
                        low[v] = low[v].min(index[w]);
                    }
                    continue;
                }

                if let Some(&(parent, _)) = call.last() {
                    low[parent] = low[parent].min(low[v]);
                }
                if low[v] == index[v] {
                    let mut comp = Vec::new();
                    while let Some(w) = stack.pop() {
                        on_stack[w.0 as usize] = false;
                        comp.push(w);
                        if w.0 as usize == v {
                            break;
                        }
                    }
                    let self_loop = comp.len() == 1
                        && self.out[v]
                            .iter()
                            .any(|a| a.other.0 as usize == v && f.allows(self.model, a.rel));
                    if comp.len() > 1 || self_loop {
                        comp.sort();
                        result.push(comp);
                    }
                }
            }
        }
        result.sort();
        result
    }

    /// Connected components, ignoring edge direction.
    pub fn components(&self) -> Vec<Vec<ConceptId>> {
        let n = self.out.len();
        let mut seen = vec![false; n];
        let mut out = Vec::new();
        let f = EdgeFilter::default();
        for start in 0..n {
            let c = self.model.concept(ConceptId(start as u32));
            if seen[start] || !c.alive || c.kind.is_relationship() {
                continue;
            }
            let mut comp = Vec::new();
            let mut queue = VecDeque::from([ConceptId(start as u32)]);
            seen[start] = true;
            while let Some(cur) = queue.pop_front() {
                comp.push(cur);
                for arc in self.neighbors(cur, Dir::Both, &f) {
                    if !seen[arc.other.0 as usize] {
                        seen[arc.other.0 as usize] = true;
                        queue.push_back(arc.other);
                    }
                }
            }
            comp.sort();
            out.push(comp);
        }
        out.sort_by_key(|c| (std::cmp::Reverse(c.len()), c.first().copied()));
        out
    }

    // ---- lookup -----------------------------------------------------------

    /// Concepts with exactly this name, case-insensitively. May be several.
    pub fn by_name(&self, name: &str) -> &[ConceptId] {
        self.by_name.get(&name.to_lowercase()).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Case-insensitive substring search over name, then documentation, then
    /// property values. Name hits sort first because that is almost always what
    /// the caller meant.
    pub fn search(&self, needle: &str, limit: usize) -> Vec<Hit> {
        let needle = needle.to_lowercase();
        let mut hits: Vec<Hit> = Vec::new();
        for (id, c) in self.model.concepts_with_ids() {
            if c.name.to_lowercase().contains(&needle) {
                hits.push(Hit { concept: id, field: MatchField::Name, snippet: c.name.clone() });
                continue;
            }
            if let Some(doc) = self.model.documentation(c.node)
                && let Some(at) = doc.to_lowercase().find(&needle)
            {
                hits.push(Hit {
                    concept: id,
                    field: MatchField::Documentation,
                    snippet: snippet(&doc, at, needle.len()),
                });
                continue;
            }
            for (k, v) in self.model.properties(c.node) {
                if let Some(at) = v.to_lowercase().find(&needle) {
                    hits.push(Hit {
                        concept: id,
                        field: MatchField::Property(k),
                        snippet: snippet(&v, at, needle.len()),
                    });
                    break;
                }
            }
        }
        hits.sort_by(|a, b| {
            let key = |h: &Hit| {
                (
                    match h.field {
                        MatchField::Name => 0,
                        MatchField::Documentation => 1,
                        MatchField::Property(_) => 2,
                    },
                    self.model.concept(h.concept).name.to_lowercase(),
                    h.concept,
                )
            };
            key(a).cmp(&key(b))
        });
        hits.truncate(limit);
        hits
    }

    pub fn stats(&self) -> Stats {
        let mut by_type: HashMap<String, usize> = HashMap::new();
        let mut by_layer: HashMap<Layer, usize> = HashMap::new();
        let mut orphans = 0;
        let mut elements = 0;
        let mut relationships = 0;

        for (id, c) in self.model.concepts_with_ids() {
            *by_type.entry(c.kind.name().to_string()).or_default() += 1;
            if c.kind.is_relationship() {
                relationships += 1;
                continue;
            }
            elements += 1;
            if let Some(l) = c.kind.layer() {
                *by_layer.entry(l).or_default() += 1;
            }
            let (i_deg, o_deg) = self.degree(id);
            if i_deg + o_deg == 0 {
                orphans += 1;
            }
        }
        Stats {
            elements,
            relationships,
            views: self.model.views().count(),
            folders: self.model.folders().count(),
            orphans,
            by_type,
            by_layer,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub concept: ConceptId,
    pub field: MatchField,
    pub snippet: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MatchField {
    Name,
    Documentation,
    Property(String),
}

impl MatchField {
    pub fn as_str(&self) -> &str {
        match self {
            MatchField::Name => "name",
            MatchField::Documentation => "documentation",
            MatchField::Property(k) => k,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Stats {
    pub elements: usize,
    pub relationships: usize,
    pub views: usize,
    pub folders: usize,
    pub orphans: usize,
    pub by_type: HashMap<String, usize>,
    pub by_layer: HashMap<Layer, usize>,
}

/// A window of text around a match, so a search result is readable without
/// dragging a whole documentation blob into the caller's context.
fn snippet(text: &str, at: usize, len: usize) -> String {
    const PAD: usize = 40;
    let start = text[..at].char_indices().rev().nth(PAD).map(|(i, _)| i).unwrap_or(0);
    let end_from = (at + len).min(text.len());
    let end =
        text[end_from..].char_indices().nth(PAD).map(|(i, _)| end_from + i).unwrap_or(text.len());
    let mut s = String::new();
    if start > 0 {
        s.push('…');
    }
    s.push_str(text[start..end].trim());
    if end < text.len() {
        s.push('…');
    }
    s.replace(['\n', '\t'], " ")
}

/// Element types, for callers building type filters from user input.
pub fn element_type(name: &str) -> Option<ElementType> {
    ElementType::from_str(name)
}
