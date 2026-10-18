//! Union-find with path compression (halving) and union by size.

#[derive(Clone, Debug)]
pub struct UnionFind {
    parent: Vec<u32>,
    size: Vec<u32>,
}

impl UnionFind {
    pub fn new(n: usize) -> Self {
        assert!(n < u32::MAX as usize, "at most 2^32 - 1 elements");
        Self {
            parent: (0..n as u32).collect(),
            size: vec![1; n],
        }
    }

    pub fn len(&self) -> usize {
        self.parent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    pub fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] as usize != x {
            let p = self.parent[x] as usize;
            self.parent[x] = self.parent[p];
            x = self.parent[x] as usize;
        }
        x
    }

    /// Merge the sets of `a` and `b`; returns the new root, or `None` when already together.
    pub fn union(&mut self, a: usize, b: usize) -> Option<usize> {
        let (mut ra, mut rb) = (self.find(a), self.find(b));
        if ra == rb {
            return None;
        }
        if self.size[ra] < self.size[rb] || (self.size[ra] == self.size[rb] && rb < ra) {
            std::mem::swap(&mut ra, &mut rb);
        }
        self.parent[rb] = ra as u32;
        self.size[ra] += self.size[rb];
        Some(ra)
    }

    pub fn same(&mut self, a: usize, b: usize) -> bool {
        self.find(a) == self.find(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unions_and_finds() {
        let mut uf = UnionFind::new(6);
        assert!(uf.union(0, 1).is_some());
        assert!(uf.union(2, 3).is_some());
        assert!(uf.union(1, 3).is_some());
        assert!(uf.union(0, 2).is_none());
        assert!(uf.same(0, 3));
        assert!(!uf.same(0, 4));
        assert_eq!(uf.len(), 6);
    }
}
