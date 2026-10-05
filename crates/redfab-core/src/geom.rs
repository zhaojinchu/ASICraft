//! Positions and directions in Minecraft's axes: +X east, +Y up, +Z south.

use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Pos {
    pub const fn new(x: i32, y: i32, z: i32) -> Pos {
        Pos { x, y, z }
    }

    pub fn offset(self, dir: Dir) -> Pos {
        let (dx, dy, dz) = dir.delta();
        Pos::new(self.x + dx, self.y + dy, self.z + dz)
    }

    pub fn up(self) -> Pos {
        self.offset(Dir::Up)
    }

    pub fn down(self) -> Pos {
        self.offset(Dir::Down)
    }
}

/// Written `x,y,z`, as in fixture and trace files.
impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{},{}", self.x, self.y, self.z)
    }
}

impl FromStr for Pos {
    type Err = String;

    fn from_str(s: &str) -> Result<Pos, String> {
        let parts: Vec<&str> = s.split(',').collect();
        if parts.len() != 3 {
            return Err(format!("not a position: {s:?}"));
        }
        let num = |p: &str| p.trim().parse::<i32>().map_err(|_| format!("not a position: {s:?}"));
        Ok(Pos::new(num(parts[0])?, num(parts[1])?, num(parts[2])?))
    }
}

/// The six directions, in the game's own enum order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Dir {
    /// The game's `Direction.values()` order.
    pub const ALL: [Dir; 6] = [Dir::Down, Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East];
    /// The game's `Direction.Plane.HORIZONTAL` order.
    pub const HORIZONTAL: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];
    /// The order in which a block update is delivered to the six neighbours.
    pub const UPDATE_ORDER: [Dir; 6] = [Dir::West, Dir::East, Dir::Down, Dir::Up, Dir::North, Dir::South];
    /// The order in which neighbours are asked to update their shape.
    pub const SHAPE_ORDER: [Dir; 6] = [Dir::West, Dir::East, Dir::North, Dir::South, Dir::Down, Dir::Up];

    pub fn delta(self) -> (i32, i32, i32) {
        match self {
            Dir::Down => (0, -1, 0),
            Dir::Up => (0, 1, 0),
            Dir::North => (0, 0, -1),
            Dir::South => (0, 0, 1),
            Dir::West => (-1, 0, 0),
            Dir::East => (1, 0, 0),
        }
    }

    pub fn opposite(self) -> Dir {
        match self {
            Dir::Down => Dir::Up,
            Dir::Up => Dir::Down,
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }

    /// The next horizontal direction clockwise, seen from above. Up and down stay put.
    pub fn clockwise(self) -> Dir {
        match self {
            Dir::North => Dir::East,
            Dir::East => Dir::South,
            Dir::South => Dir::West,
            Dir::West => Dir::North,
            other => other,
        }
    }

    pub fn is_horizontal(self) -> bool {
        !matches!(self, Dir::Up | Dir::Down)
    }

    pub fn name(self) -> &'static str {
        match self {
            Dir::Down => "down",
            Dir::Up => "up",
            Dir::North => "north",
            Dir::South => "south",
            Dir::West => "west",
            Dir::East => "east",
        }
    }

    pub fn from_name(name: &str) -> Option<Dir> {
        Dir::ALL.into_iter().find(|d| d.name() == name)
    }
}
