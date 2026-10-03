//! Which window edge the pointer is on, for resizing a borderless window.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

/// `pos` and `size` are in points inside the window; `border` is the grab width.
/// Corners get a grab area twice as long so they are easy to hit.
pub fn edge_at(pos: (f32, f32), size: (f32, f32), border: f32) -> Option<Edge> {
    let (x, y) = pos;
    let (w, h) = size;
    if x < 0.0 || y < 0.0 || x > w || y > h {
        return None;
    }
    let corner = border * 2.0;
    let left = x < border;
    let right = x > w - border;
    let top = y < border;
    let bottom = y > h - border;
    let near_left = x < corner;
    let near_right = x > w - corner;
    let near_top = y < corner;
    let near_bottom = y > h - corner;
    Some(match () {
        _ if (top && near_left) || (left && near_top) => Edge::NorthWest,
        _ if (top && near_right) || (right && near_top) => Edge::NorthEast,
        _ if (bottom && near_left) || (left && near_bottom) => Edge::SouthWest,
        _ if (bottom && near_right) || (right && near_bottom) => Edge::SouthEast,
        _ if top => Edge::North,
        _ if bottom => Edge::South,
        _ if left => Edge::West,
        _ if right => Edge::East,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (f32, f32) = (800.0, 600.0);

    #[test]
    fn edges_and_corners() {
        assert_eq!(edge_at((400.0, 2.0), SIZE, 6.0), Some(Edge::North));
        assert_eq!(edge_at((400.0, 598.0), SIZE, 6.0), Some(Edge::South));
        assert_eq!(edge_at((2.0, 300.0), SIZE, 6.0), Some(Edge::West));
        assert_eq!(edge_at((798.0, 300.0), SIZE, 6.0), Some(Edge::East));
        assert_eq!(edge_at((2.0, 2.0), SIZE, 6.0), Some(Edge::NorthWest));
        assert_eq!(edge_at((10.0, 2.0), SIZE, 6.0), Some(Edge::NorthWest), "long corner");
        assert_eq!(edge_at((798.0, 10.0), SIZE, 6.0), Some(Edge::NorthEast));
        assert_eq!(edge_at((2.0, 598.0), SIZE, 6.0), Some(Edge::SouthWest));
        assert_eq!(edge_at((798.0, 598.0), SIZE, 6.0), Some(Edge::SouthEast));
    }

    #[test]
    fn inside_and_outside() {
        assert_eq!(edge_at((400.0, 300.0), SIZE, 6.0), None);
        assert_eq!(edge_at((-1.0, 300.0), SIZE, 6.0), None);
    }
}
