//! The closed set of blocks redfab understands, with the game's state strings.
//!
//! A state string looks like `minecraft:redstone_wire[east=side,north=none,power=15,south=none,west=side]`.
//! `Display` always writes every property, sorted by name, which is the canonical
//! form used in trace files. Parsing accepts missing properties and fills in the
//! game's defaults.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use crate::geom::Dir;

/// How one side of a redstone wire connects.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Side {
    None,
    Side,
    Up,
}

impl Side {
    pub fn is_connected(self) -> bool {
        self != Side::None
    }

    fn name(self) -> &'static str {
        match self {
            Side::None => "none",
            Side::Side => "side",
            Side::Up => "up",
        }
    }

    fn from_name(s: &str) -> Option<Side> {
        match s {
            "none" => Some(Side::None),
            "side" => Some(Side::Side),
            "up" => Some(Side::Up),
            _ => None,
        }
    }
}

/// A redstone wire's four sides and power.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Wire {
    pub power: u8,
    pub north: Side,
    pub east: Side,
    pub south: Side,
    pub west: Side,
}

impl Wire {
    pub const DOT: Wire = Wire { power: 0, north: Side::None, east: Side::None, south: Side::None, west: Side::None };

    pub fn side(&self, dir: Dir) -> Side {
        match dir {
            Dir::North => self.north,
            Dir::East => self.east,
            Dir::South => self.south,
            Dir::West => self.west,
            Dir::Up | Dir::Down => Side::None,
        }
    }

    pub fn with_side(mut self, dir: Dir, side: Side) -> Wire {
        match dir {
            Dir::North => self.north = side,
            Dir::East => self.east = side,
            Dir::South => self.south = side,
            Dir::West => self.west = side,
            Dir::Up | Dir::Down => {}
        }
        self
    }

    pub fn is_dot(&self) -> bool {
        Dir::HORIZONTAL.iter().all(|&d| !self.side(d).is_connected())
    }

    pub fn is_cross(&self) -> bool {
        Dir::HORIZONTAL.iter().all(|&d| self.side(d).is_connected())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Block {
    Air,
    /// A plain conductive full block.
    Stone,
    /// A full block that does not conduct redstone but supports dust and torches.
    Glass,
    RedstoneBlock,
    Wire(Wire),
    /// A redstone torch standing on the block below.
    Torch {
        lit: bool,
    },
    /// A redstone torch on the side of a block; `facing` points away from that block.
    WallTorch {
        facing: Dir,
        lit: bool,
    },
    Lamp {
        lit: bool,
    },
    /// A repeater. `facing` points at its input; it outputs on the opposite side.
    /// `delay` is 1 to 4 (each step is two game ticks).
    Repeater {
        facing: Dir,
        delay: u8,
        locked: bool,
        powered: bool,
    },
    /// A lever. `face` says which kind of surface it is mounted on.
    Lever {
        face: Face,
        facing: Dir,
        powered: bool,
    },
}

/// Where a lever is mounted.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Face {
    Floor,
    Wall,
    Ceiling,
}

impl Face {
    fn name(self) -> &'static str {
        match self {
            Face::Floor => "floor",
            Face::Wall => "wall",
            Face::Ceiling => "ceiling",
        }
    }

    fn from_name(s: &str) -> Option<Face> {
        match s {
            "floor" => Some(Face::Floor),
            "wall" => Some(Face::Wall),
            "ceiling" => Some(Face::Ceiling),
            _ => None,
        }
    }
}

impl Block {
    /// The block id without properties, e.g. `minecraft:redstone_wire`.
    pub fn name(&self) -> &'static str {
        match self {
            Block::Air => "minecraft:air",
            Block::Stone => "minecraft:stone",
            Block::Glass => "minecraft:glass",
            Block::RedstoneBlock => "minecraft:redstone_block",
            Block::Wire(_) => "minecraft:redstone_wire",
            Block::Torch { .. } => "minecraft:redstone_torch",
            Block::WallTorch { .. } => "minecraft:redstone_wall_torch",
            Block::Lamp { .. } => "minecraft:redstone_lamp",
            Block::Lever { .. } => "minecraft:lever",
            Block::Repeater { .. } => "minecraft:repeater",
        }
    }

    /// For a lever: the direction from the lever to the block it is mounted on.
    pub fn lever_mount(&self) -> Option<Dir> {
        match self {
            Block::Lever { face: Face::Floor, .. } => Some(Dir::Down),
            Block::Lever { face: Face::Ceiling, .. } => Some(Dir::Up),
            Block::Lever { face: Face::Wall, facing, .. } => Some(facing.opposite()),
            _ => None,
        }
    }

    pub fn is_air(&self) -> bool {
        *self == Block::Air
    }

    /// The game's `isRedstoneConductor`: an opaque full block that passes power on.
    pub fn is_conductor(&self) -> bool {
        matches!(self, Block::Stone | Block::Lamp { .. })
    }

    /// Has a solid top face, so dust or a torch can sit on it.
    pub fn is_full_block(&self) -> bool {
        matches!(self, Block::Stone | Block::Glass | Block::RedstoneBlock | Block::Lamp { .. })
    }

    /// The game's `isSignalSource`.
    pub fn is_signal_source(&self) -> bool {
        matches!(
            self,
            Block::RedstoneBlock
                | Block::Wire(_)
                | Block::Torch { .. }
                | Block::WallTorch { .. }
                | Block::Lever { .. }
                | Block::Repeater { .. }
        )
    }

    pub fn is_torch(&self) -> bool {
        matches!(self, Block::Torch { .. } | Block::WallTorch { .. })
    }

    pub fn as_wire(&self) -> Option<Wire> {
        match self {
            Block::Wire(w) => Some(*w),
            _ => None,
        }
    }
}

fn bool_name(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

impl Block {
    /// This block turned `quarter_turns` clockwise about the vertical axis
    /// (north → east → south → west).
    pub fn rotated(&self, quarter_turns: u8) -> Block {
        let turn = |d: Dir| (0..quarter_turns % 4).fold(d, |d, _| d.clockwise());
        match *self {
            Block::Wire(w) => {
                let sides = [w.north, w.east, w.south, w.west];
                let shift = (quarter_turns % 4) as usize;
                // side i moves to position (i + shift) mod 4 in the n, e, s, w cycle
                let at = |i: usize| sides[(i + 4 - shift) % 4];
                Block::Wire(Wire { power: w.power, north: at(0), east: at(1), south: at(2), west: at(3) })
            }
            Block::WallTorch { facing, lit } => Block::WallTorch { facing: turn(facing), lit },
            Block::Repeater { facing, delay, locked, powered } => {
                Block::Repeater { facing: turn(facing), delay, locked, powered }
            }
            Block::Lever { face, facing, powered } => Block::Lever { face, facing: turn(facing), powered },
            other => other,
        }
    }

    /// Every property of this block's state with its value, sorted by name.
    pub fn properties(&self) -> Vec<(&'static str, String)> {
        match self {
            Block::Wire(w) => vec![
                ("east", w.east.name().to_string()),
                ("north", w.north.name().to_string()),
                ("power", w.power.to_string()),
                ("south", w.south.name().to_string()),
                ("west", w.west.name().to_string()),
            ],
            Block::Torch { lit } | Block::Lamp { lit } => {
                vec![("lit", bool_name(*lit).to_string())]
            }
            Block::WallTorch { facing, lit } => {
                vec![("facing", facing.name().to_string()), ("lit", bool_name(*lit).to_string())]
            }
            Block::Repeater { facing, delay, locked, powered } => vec![
                ("delay", delay.to_string()),
                ("facing", facing.name().to_string()),
                ("locked", bool_name(*locked).to_string()),
                ("powered", bool_name(*powered).to_string()),
            ],
            Block::Lever { face, facing, powered } => vec![
                ("face", face.name().to_string()),
                ("facing", facing.name().to_string()),
                ("powered", bool_name(*powered).to_string()),
            ],
            Block::Air | Block::Stone | Block::Glass | Block::RedstoneBlock => Vec::new(),
        }
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())?;
        let props = self.properties();
        if !props.is_empty() {
            let items: Vec<String> = props.iter().map(|(k, v)| format!("{k}={v}")).collect();
            write!(f, "[{}]", items.join(","))?;
        }
        Ok(())
    }
}

/// A block-state string split into name and properties, before it is typed.
pub struct StateText {
    pub name: String,
    pub props: BTreeMap<String, String>,
}

impl FromStr for StateText {
    type Err = String;

    fn from_str(s: &str) -> Result<StateText, String> {
        let s = s.trim();
        let (name, rest) = match s.find('[') {
            Some(i) => (&s[..i], Some(&s[i + 1..])),
            None => (s, None),
        };
        let name = if name.contains(':') { name.to_string() } else { format!("minecraft:{name}") };
        let mut props = BTreeMap::new();
        if let Some(rest) = rest {
            let inner = rest.strip_suffix(']').ok_or_else(|| format!("missing ']' in {s:?}"))?;
            for item in inner.split(',').filter(|i| !i.is_empty()) {
                let (k, v) = item.split_once('=').ok_or_else(|| format!("bad property {item:?} in {s:?}"))?;
                props.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        Ok(StateText { name, props })
    }
}

impl FromStr for Block {
    type Err = String;

    fn from_str(s: &str) -> Result<Block, String> {
        let text: StateText = s.parse()?;
        let mut props = text.props;
        let mut take = |key: &str, default: &str| props.remove(key).unwrap_or_else(|| default.to_string());
        let parse_bool = |v: String| match v.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(format!("bad boolean {v:?} in {s:?}")),
        };
        let block = match text.name.as_str() {
            "minecraft:air" => Block::Air,
            "minecraft:stone" => Block::Stone,
            "minecraft:glass" => Block::Glass,
            "minecraft:redstone_block" => Block::RedstoneBlock,
            "minecraft:redstone_wire" => {
                let side = |v: String| Side::from_name(&v).ok_or_else(|| format!("bad wire side {v:?} in {s:?}"));
                let power: u8 = take("power", "0").parse().map_err(|_| format!("bad power in {s:?}"))?;
                if power > 15 {
                    return Err(format!("power out of range in {s:?}"));
                }
                Block::Wire(Wire {
                    power,
                    north: side(take("north", "none"))?,
                    east: side(take("east", "none"))?,
                    south: side(take("south", "none"))?,
                    west: side(take("west", "none"))?,
                })
            }
            "minecraft:redstone_torch" => Block::Torch { lit: parse_bool(take("lit", "true"))? },
            "minecraft:redstone_wall_torch" => {
                let facing = take("facing", "north");
                let facing = Dir::from_name(&facing)
                    .filter(|d| d.is_horizontal())
                    .ok_or_else(|| format!("bad facing {facing:?} in {s:?}"))?;
                Block::WallTorch { facing, lit: parse_bool(take("lit", "true"))? }
            }
            "minecraft:redstone_lamp" => Block::Lamp { lit: parse_bool(take("lit", "false"))? },
            "minecraft:repeater" => {
                let facing = take("facing", "north");
                let facing = Dir::from_name(&facing)
                    .filter(|d| d.is_horizontal())
                    .ok_or_else(|| format!("bad facing {facing:?} in {s:?}"))?;
                let delay: u8 = take("delay", "1").parse().map_err(|_| format!("bad delay in {s:?}"))?;
                if !(1..=4).contains(&delay) {
                    return Err(format!("repeater delay must be 1 to 4 in {s:?}"));
                }
                let locked = parse_bool(take("locked", "false"))?;
                Block::Repeater { facing, delay, locked, powered: parse_bool(take("powered", "false"))? }
            }
            "minecraft:lever" => {
                let face = take("face", "wall");
                let face = Face::from_name(&face).ok_or_else(|| format!("bad face {face:?} in {s:?}"))?;
                let facing = take("facing", "north");
                let facing = Dir::from_name(&facing)
                    .filter(|d| d.is_horizontal())
                    .ok_or_else(|| format!("bad facing {facing:?} in {s:?}"))?;
                Block::Lever { face, facing, powered: parse_bool(take("powered", "false"))? }
            }
            other => return Err(format!("unsupported block {other}")),
        };
        if let Some(extra) = props.keys().next() {
            return Err(format!("unknown property {extra:?} in {s:?}"));
        }
        Ok(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_canonical_and_round_trips() {
        for s in [
            "minecraft:air",
            "minecraft:stone",
            "minecraft:glass",
            "minecraft:redstone_block",
            "minecraft:redstone_wire[east=side,north=up,power=7,south=none,west=side]",
            "minecraft:redstone_torch[lit=false]",
            "minecraft:redstone_wall_torch[facing=west,lit=true]",
            "minecraft:redstone_lamp[lit=true]",
            "minecraft:lever[face=floor,facing=east,powered=false]",
            "minecraft:repeater[delay=3,facing=west,locked=true,powered=false]",
        ] {
            let block: Block = s.parse().unwrap();
            assert_eq!(block.to_string(), s);
        }
    }

    #[test]
    fn missing_properties_take_the_games_defaults() {
        assert_eq!("redstone_torch".parse::<Block>().unwrap(), Block::Torch { lit: true });
        assert_eq!("minecraft:redstone_lamp".parse::<Block>().unwrap(), Block::Lamp { lit: false });
        assert_eq!("minecraft:redstone_wire".parse::<Block>().unwrap(), Block::Wire(Wire::DOT));
    }

    #[test]
    fn rotation_turns_facings_and_wire_sides_clockwise() {
        let turned = |s: &str, n: u8| s.parse::<Block>().unwrap().rotated(n).to_string();
        assert_eq!(
            turned("minecraft:repeater[facing=north]", 1),
            "minecraft:repeater[delay=1,facing=east,locked=false,powered=false]"
        );
        assert_eq!(
            turned("minecraft:redstone_wall_torch[facing=west]", 1),
            "minecraft:redstone_wall_torch[facing=north,lit=true]"
        );
        assert_eq!(
            turned("minecraft:redstone_wire[east=side,north=up]", 1),
            "minecraft:redstone_wire[east=up,north=none,power=0,south=side,west=none]"
        );
        assert_eq!(
            turned("minecraft:lever[face=floor,facing=south]", 2),
            "minecraft:lever[face=floor,facing=north,powered=false]"
        );
        assert_eq!(turned("minecraft:stone", 3), "minecraft:stone");
    }

    #[test]
    fn unknown_blocks_and_properties_are_errors() {
        assert!("minecraft:comparator".parse::<Block>().is_err());
        assert!("minecraft:repeater[delay=5]".parse::<Block>().is_err());
        assert!("minecraft:stone[foo=bar]".parse::<Block>().is_err());
        assert!("minecraft:redstone_wire[power=16]".parse::<Block>().is_err());
    }
}
