// Grouping linked items: a group is a connected component of the links, so items join through any
// chain of links, not only direct ones.

use std::collections::HashMap;

// The groups of `0..len` that `links` connect, each in index order, ordered by their first index.
// An item with no links is a group alone.
pub fn components(len: usize, links: impl IntoIterator<Item = (usize, usize)>) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..len).collect();
    for (i, j) in links {
        let (a, b) = (root(&mut parent, i), root(&mut parent, j));
        parent[a] = b;
    }

    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut group_of_root: HashMap<usize, usize> = HashMap::new();
    for i in 0..len {
        let r = root(&mut parent, i);
        let g = *group_of_root.entry(r).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(i);
    }
    groups
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

#[cfg(test)]
mod tests {
    use super::components;

    #[test]
    fn unlinked_items_are_alone() {
        assert_eq!(components(3, []), [vec![0], vec![1], vec![2]]);
    }

    #[test]
    fn a_chain_of_links_is_one_group() {
        assert_eq!(components(4, [(3, 2), (0, 3)]), [vec![0, 2, 3], vec![1]]);
    }

    #[test]
    fn links_in_either_direction_and_twice_are_the_same() {
        assert_eq!(
            components(3, [(0, 1), (1, 0), (1, 0), (2, 2)]),
            [vec![0, 1], vec![2]]
        );
    }

    #[test]
    fn nothing_makes_no_groups() {
        assert!(components(0, []).is_empty());
    }
}
