//! Tiled panes: a binary tree of horizontal and vertical splits.

use ratatui::layout::Rect;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Leaf(usize),
    Split {
        /// True: side by side (vertical split); false: one above the other.
        side_by_side: bool,
        /// Share of the first child, as a percentage.
        ratio: u16,
        a: Box<Node>,
        b: Box<Node>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Node {
    pub fn split(a: Node, b: Node, side_by_side: bool, ratio: u16) -> Node {
        Node::Split {
            side_by_side,
            ratio,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    /// Rectangle of each pane.
    pub fn layout(&self, area: Rect, out: &mut Vec<(usize, Rect)>) {
        match self {
            Node::Leaf(id) => out.push((*id, area)),
            Node::Split {
                side_by_side,
                ratio,
                a,
                b,
            } => {
                let (ra, rb) = cut(area, *side_by_side, *ratio);
                a.layout(ra, out);
                b.layout(rb, out);
            }
        }
    }

    pub fn leaves(&self) -> Vec<usize> {
        match self {
            Node::Leaf(id) => vec![*id],
            Node::Split { a, b, .. } => {
                let mut v = a.leaves();
                v.extend(b.leaves());
                v
            }
        }
    }

    /// Replaces pane `target` with a `target | new` split.
    pub fn split_leaf(&mut self, target: usize, new: usize, side_by_side: bool) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                *self = Node::split(Node::Leaf(target), Node::Leaf(new), side_by_side, 50);
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => {
                a.split_leaf(target, new, side_by_side) || b.split_leaf(target, new, side_by_side)
            }
        }
    }

    /// Removes a pane: its sibling takes its place. The last one stays.
    pub fn close(&mut self, target: usize) -> bool {
        let Node::Split { a, b, .. } = self else {
            return false;
        };
        if **a == Node::Leaf(target) {
            *self = (**b).clone();
            return true;
        }
        if **b == Node::Leaf(target) {
            *self = (**a).clone();
            return true;
        }
        a.close(target) || b.close(target)
    }

    /// Grows (delta > 0) or shrinks the pane within the closest parent split
    /// that has the right orientation.
    pub fn resize(&mut self, target: usize, delta: i16, side_by_side_wanted: bool) -> bool {
        match self {
            Node::Leaf(_) => false,
            Node::Split {
                side_by_side,
                ratio,
                a,
                b,
            } => {
                if a.resize(target, delta, side_by_side_wanted)
                    || b.resize(target, delta, side_by_side_wanted)
                {
                    return true;
                }
                let in_a = a.leaves().contains(&target);
                let in_b = b.leaves().contains(&target);
                if *side_by_side == side_by_side_wanted && (in_a || in_b) {
                    let d = if in_a { delta } else { -delta };
                    *ratio = (*ratio as i16 + d).clamp(10, 90) as u16;
                    return true;
                }
                false
            }
        }
    }
}

fn cut(area: Rect, side_by_side: bool, ratio: u16) -> (Rect, Rect) {
    if side_by_side {
        let w = (u32::from(area.width) * u32::from(ratio) / 100) as u16;
        (
            Rect { width: w, ..area },
            Rect {
                x: area.x + w,
                width: area.width - w,
                ..area
            },
        )
    } else {
        let h = (u32::from(area.height) * u32::from(ratio) / 100) as u16;
        (
            Rect { height: h, ..area },
            Rect {
                y: area.y + h,
                height: area.height - h,
                ..area
            },
        )
    }
}

/// Neighboring pane in a direction, based on geometry.
pub fn neighbor(rects: &[(usize, Rect)], from: usize, dir: Dir) -> Option<usize> {
    let (_, cur) = rects.iter().find(|(id, _)| *id == from)?;
    let center = |r: &Rect| {
        (
            i32::from(r.x) * 2 + i32::from(r.width),
            i32::from(r.y) * 2 + i32::from(r.height),
        )
    };
    let (cx, cy) = center(cur);
    rects
        .iter()
        .filter(|(id, _)| *id != from)
        .filter(|(_, r)| match dir {
            Dir::Left => r.x + r.width <= cur.x,
            Dir::Right => r.x >= cur.x + cur.width,
            Dir::Up => r.y + r.height <= cur.y,
            Dir::Down => r.y >= cur.y + cur.height,
        })
        .min_by_key(|(_, r)| {
            let (x, y) = center(r);
            (x - cx).abs() + (y - cy).abs()
        })
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_close_and_resize() {
        let mut root = Node::Leaf(0);
        assert!(root.split_leaf(0, 1, false));
        assert!(root.split_leaf(1, 2, true));
        assert_eq!(root.leaves(), vec![0, 1, 2]);
        let mut rects = Vec::new();
        root.layout(Rect::new(0, 0, 80, 24), &mut rects);
        assert_eq!(rects[0].1, Rect::new(0, 0, 80, 12));
        assert_eq!(rects[1].1, Rect::new(0, 12, 40, 12));
        assert_eq!(rects[2].1, Rect::new(40, 12, 40, 12));
        assert_eq!(neighbor(&rects, 1, Dir::Right), Some(2));
        assert_eq!(neighbor(&rects, 2, Dir::Up), Some(0));
        assert_eq!(neighbor(&rects, 0, Dir::Up), None);
        assert!(root.resize(0, 10, false));
        let mut rects = Vec::new();
        root.layout(Rect::new(0, 0, 80, 24), &mut rects);
        assert_eq!(rects[0].1.height, 14);
        assert!(root.close(1));
        assert_eq!(root.leaves(), vec![0, 2]);
        assert!(root.close(2));
        assert_eq!(root, Node::Leaf(0));
        assert!(!root.close(0));
    }
}
