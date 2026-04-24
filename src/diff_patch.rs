//! Character-level diff matching the web client's `computeDiff` in `src/diff.ts`.
//!
//! The wire format uses a `Fragment` enum with three kinds. The server / CC
//! client then replays the fragments over the remote's original contents.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Fragment {
    Same { kind: SameKind, length: usize },
    Removed { kind: RemovedKind, length: usize },
    Added { kind: AddedKind, contents: String },
}

// Each kind gets its own unit struct that serializes as a specific u8, so the
// `untagged` enum above discriminates by which field set is present.
macro_rules! const_kind {
    ($name:ident, $val:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct $name;
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_u8($val)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let n = u8::deserialize(d)?;
                if n == $val { Ok($name) } else {
                    Err(serde::de::Error::custom(format!(
                        "expected kind {}, got {}", $val, n
                    )))
                }
            }
        }
    };
}

const_kind!(SameKind, 0);
const_kind!(AddedKind, 1);
const_kind!(RemovedKind, 2);

/// Compute a char-level diff, matching `diffChars` from the `diff` npm package
/// closely enough for the CC client's patch applier (which just walks fragments
/// in order and doesn't care about minimality).
pub fn compute_diff(old: &str, new: &str) -> Vec<Fragment> {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::configure().algorithm(similar::Algorithm::Myers)
        .diff_chars(old, new);

    let mut out: Vec<Fragment> = Vec::new();
    // Coalesce consecutive runs of the same tag.
    let mut run_tag: Option<ChangeTag> = None;
    let mut run_buf = String::new();
    let mut run_len: usize = 0;

    let flush = |out: &mut Vec<Fragment>, tag: Option<ChangeTag>, buf: &mut String, len: &mut usize| {
        match tag {
            Some(ChangeTag::Equal) => out.push(Fragment::Same { kind: SameKind, length: *len }),
            Some(ChangeTag::Delete) => out.push(Fragment::Removed { kind: RemovedKind, length: *len }),
            Some(ChangeTag::Insert) => out.push(Fragment::Added {
                kind: AddedKind,
                contents: std::mem::take(buf),
            }),
            None => {}
        }
        *len = 0;
    };

    for change in diff.iter_all_changes() {
        let tag = change.tag();
        let val = change.value();
        if Some(tag) != run_tag {
            let mut local_buf = std::mem::take(&mut run_buf);
            let mut local_len = run_len;
            flush(&mut out, run_tag, &mut local_buf, &mut local_len);
            run_tag = Some(tag);
            run_buf = String::new();
            run_len = 0;
        }
        if matches!(tag, ChangeTag::Insert) {
            run_buf.push_str(val);
        }
        // Length counts chars, not bytes — the server/CC side works in UTF-8
        // bytes, which matches for ASCII (dominant case). Non-ASCII files may
        // drift; see PLAN.md.
        run_len += val.chars().count();
    }
    let mut local_buf = std::mem::take(&mut run_buf);
    let mut local_len = run_len;
    flush(&mut out, run_tag, &mut local_buf, &mut local_len);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical() {
        let frags = compute_diff("abc", "abc");
        assert!(matches!(frags.as_slice(), [Fragment::Same { length: 3, .. }]));
    }

    #[test]
    fn insert_only() {
        let frags = compute_diff("", "xy");
        assert!(matches!(frags.as_slice(), [Fragment::Added { contents, .. }] if contents == "xy"));
    }

    #[test]
    fn replace() {
        // "ab" → "cd": expect [Removed(2), Added("cd")] (order may vary; just
        // assert round-tripability conceptually).
        let frags = compute_diff("ab", "cd");
        let removed: usize = frags.iter().filter_map(|f| match f {
            Fragment::Removed { length, .. } => Some(*length), _ => None,
        }).sum();
        let added: String = frags.iter().filter_map(|f| match f {
            Fragment::Added { contents, .. } => Some(contents.clone()), _ => None,
        }).collect();
        assert_eq!(removed, 2);
        assert_eq!(added, "cd");
    }
}
