//! A compact, immutable trie over `char` sequences.
//!
//! Built with [`TrieBuilder`] and then frozen into flat arrays: every node's outgoing edges are a
//! sorted run in one shared edge array, found by binary search. Node and entry indices are
//! stored as `u32` to keep the 10,000-snippet trie small.

/// One frozen node: its run of edges and the entry that ends here, if any.
#[derive(Debug, Clone, Copy)]
struct Node {
    edge_start: u32,
    edge_len: u32,
    entry: Option<u32>,
}

/// An immutable trie. Node 0 is the root.
#[derive(Debug, Clone)]
pub(super) struct Trie {
    nodes: Box<[Node]>,
    edges: Box<[(char, u32)]>,
}

/// A node while building: sorted `(char, child)` edges and the entry index ending here.
type BuildNode = (Vec<(char, usize)>, Option<usize>);

/// Mutable form used while building.
#[derive(Debug, Default)]
pub(super) struct TrieBuilder {
    nodes: Vec<BuildNode>,
}

/// Converts an index to `u32`.
///
/// # Panics
///
/// Panics past `u32::MAX`, which needs more than 22 million 64-character abbreviations.
fn small(i: usize) -> u32 {
    u32::try_from(i).expect("matcher trie index exceeds u32::MAX")
}

impl TrieBuilder {
    pub(super) fn new() -> Self {
        Self {
            nodes: vec![(Vec::new(), None)],
        }
    }

    /// Inserts `key` with `entry`. Returns the entry already stored under `key`, if any, and
    /// leaves it in place.
    pub(super) fn insert(
        &mut self,
        key: impl IntoIterator<Item = char>,
        entry: usize,
    ) -> Option<usize> {
        let mut node = 0;
        for c in key {
            let next_free = self.nodes.len();
            let edges = &mut self.nodes[node].0;
            node = match edges.binary_search_by_key(&c, |e| e.0) {
                Ok(i) => edges[i].1,
                Err(i) => {
                    edges.insert(i, (c, next_free));
                    self.nodes.push((Vec::new(), None));
                    next_free
                }
            };
        }
        let slot = &mut self.nodes[node].1;
        if slot.is_some() {
            return *slot;
        }
        *slot = Some(entry);
        None
    }

    /// Freezes the trie.
    pub(super) fn freeze(self) -> Trie {
        let mut nodes = Vec::with_capacity(self.nodes.len());
        let mut edges = Vec::with_capacity(self.nodes.len().saturating_sub(1));
        for (out, entry) in self.nodes {
            nodes.push(Node {
                edge_start: small(edges.len()),
                edge_len: small(out.len()),
                entry: entry.map(small),
            });
            edges.extend(out.into_iter().map(|(c, child)| (c, small(child))));
        }
        Trie {
            nodes: nodes.into_boxed_slice(),
            edges: edges.into_boxed_slice(),
        }
    }
}

impl Trie {
    /// The root node.
    pub(super) const ROOT: u32 = 0;

    /// The child of `node` along `c`.
    pub(super) fn child(&self, node: u32, c: char) -> Option<u32> {
        let n = self.nodes[node as usize];
        let start = n.edge_start as usize;
        let run = &self.edges[start..start + n.edge_len as usize];
        run.binary_search_by_key(&c, |e| e.0).ok().map(|i| run[i].1)
    }

    /// The entry ending at `node`, if any.
    pub(super) fn entry(&self, node: u32) -> Option<usize> {
        self.nodes[node as usize].entry.map(|e| e as usize)
    }

    /// Number of nodes, root included.
    #[cfg(test)]
    pub(super) fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{Trie, TrieBuilder};

    fn walk(t: &Trie, key: &str) -> Option<u32> {
        key.chars().try_fold(Trie::ROOT, |n, c| t.child(n, c))
    }

    #[test]
    fn insert_find_and_duplicates() {
        let mut b = TrieBuilder::new();
        assert_eq!(b.insert("abc".chars(), 0), None);
        assert_eq!(b.insert("abd".chars(), 1), None);
        assert_eq!(b.insert("ab".chars(), 2), None);
        assert_eq!(b.insert("abc".chars(), 3), Some(0));
        let t = b.freeze();
        assert_eq!(t.node_count(), 5);
        assert_eq!(walk(&t, "abc").and_then(|n| t.entry(n)), Some(0));
        assert_eq!(walk(&t, "abd").and_then(|n| t.entry(n)), Some(1));
        assert_eq!(walk(&t, "ab").and_then(|n| t.entry(n)), Some(2));
        assert_eq!(walk(&t, "a").and_then(|n| t.entry(n)), None);
        assert_eq!(walk(&t, "abx"), None);
    }
}
