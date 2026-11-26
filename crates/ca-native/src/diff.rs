//! What the agent changed in a file, line by line: the longest run of lines both have kept, the rest removed or
//! added, as VS Code's inline diff shows them.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Same,
    Removed,
    Added,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub side: Side,
    /// The line's number before and after (from 1), where it has one.
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: String,
}

/// The most lines a side may have for the lines kept to be found (the work grows as their product); past it the
/// file is said as all removed, then all added.
const MOST: usize = 4000;

pub fn lines(before: &str, after: &str) -> Vec<Line> {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let (n, m) = (a.len(), b.len());
    let mut out = Vec::with_capacity(n.max(m));
    let removed = |i: usize, out: &mut Vec<Line>| {
        out.push(Line { side: Side::Removed, old: Some(i + 1), new: None, text: a[i].to_string() })
    };
    let added = |j: usize, out: &mut Vec<Line>| {
        out.push(Line { side: Side::Added, old: None, new: Some(j + 1), text: b[j].to_string() })
    };
    if n > MOST || m > MOST {
        (0..n).for_each(|i| removed(i, &mut out));
        (0..m).for_each(|j| added(j, &mut out));
        return out;
    }
    // The longest common run of lines from each place on (a table of (n+1)·(m+1)).
    let w = m + 1;
    let mut keep = vec![0u16; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            keep[i * w + j] = if a[i] == b[j] {
                keep[(i + 1) * w + j + 1].saturating_add(1)
            } else {
                keep[(i + 1) * w + j].max(keep[i * w + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(Line { side: Side::Same, old: Some(i + 1), new: Some(j + 1), text: a[i].to_string() });
            i += 1;
            j += 1;
        } else if keep[(i + 1) * w + j] >= keep[i * w + j + 1] {
            removed(i, &mut out);
            i += 1;
        } else {
            added(j, &mut out);
            j += 1;
        }
    }
    (i..n).for_each(|i| removed(i, &mut out));
    (j..m).for_each(|j| added(j, &mut out));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sides(d: &[Line]) -> String {
        d.iter()
            .map(|l| match l.side {
                Side::Same => ' ',
                Side::Removed => '-',
                Side::Added => '+',
            })
            .collect()
    }

    #[test]
    fn the_lines_kept_removed_and_added() {
        let d = lines("a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!(sides(&d), " -+ +");
        assert_eq!((d[2].text.as_str(), d[2].new, d[2].old), ("B", Some(2), None));
        assert_eq!(d[4].new, Some(4));
        assert_eq!(sides(&lines("", "x\ny")), "++");
        assert_eq!(sides(&lines("x\n", "")), "-");
    }
}
