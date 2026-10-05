//! A minimal writer for Minecraft's NBT binary format (big-endian).
//! Only the tag types structure files need.

pub enum Tag {
    Int(i32),
    Str(String),
    /// All elements must be of one type. An empty list is written with element type "end".
    List(Vec<Tag>),
    Compound(Vec<(String, Tag)>),
}

impl Tag {
    fn id(&self) -> u8 {
        match self {
            Tag::Int(_) => 3,
            Tag::Str(_) => 8,
            Tag::List(_) => 9,
            Tag::Compound(_) => 10,
        }
    }

    fn write_payload(&self, out: &mut Vec<u8>) {
        match self {
            Tag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
            Tag::Str(s) => write_string(s, out),
            Tag::List(items) => {
                out.push(items.first().map_or(0, Tag::id));
                out.extend_from_slice(&(items.len() as i32).to_be_bytes());
                for item in items {
                    debug_assert_eq!(item.id(), items[0].id(), "NBT lists hold one type");
                    item.write_payload(out);
                }
            }
            Tag::Compound(entries) => {
                for (name, tag) in entries {
                    out.push(tag.id());
                    write_string(name, out);
                    tag.write_payload(out);
                }
                out.push(0);
            }
        }
    }

    pub fn compound(entries: Vec<(&str, Tag)>) -> Tag {
        Tag::Compound(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn ints(values: [i32; 3]) -> Tag {
        Tag::List(values.into_iter().map(Tag::Int).collect())
    }
}

fn write_string(s: &str, out: &mut Vec<u8>) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// Serializes a compound as a complete NBT document (root tag with an empty name).
pub fn write_root(root: &Tag) -> Vec<u8> {
    let mut out = vec![root.id(), 0, 0];
    root.write_payload(&mut out);
    out
}
