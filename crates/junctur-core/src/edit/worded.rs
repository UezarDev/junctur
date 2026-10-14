//! Enums whose every variant is one word, the same on the edit list's `captions` line, in the
//! config file and over IPC: the word is written once, beside its variant, and serde's name,
//! `word()`, `from_word()` and `ALL` all come from it. A macro, since each enum is otherwise its
//! own hand-kept list three times over.

/// `worded! { #[derive(..)] pub enum E { #[default] A = "a", B = "b" } }`. The enum still
/// derives `Serialize`/`Deserialize` itself; the macro adds each variant's `serde(rename)`.
macro_rules! worded {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$vmeta:meta])* $variant:ident = $word:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $($(#[$vmeta])* #[serde(rename = $word)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// Its word, on the `captions` line and in the config file alike.
            pub fn word(self) -> &'static str {
                match self {
                    $($name::$variant => $word),+
                }
            }

            pub fn from_word(w: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|v| v.word() == w)
            }
        }
    };
}

pub(crate) use worded;

#[cfg(test)]
mod tests {
    use super::super::{Animation, Case, CensorStyle, ExistingClips, Layout};

    /// Serde's name is the word, so the config file and IPC say what the `captions` line says.
    fn same_as_serde<T: serde::Serialize + Copy>(all: &[T], word: fn(T) -> &'static str) {
        for &v in all {
            assert_eq!(
                serde_json::to_string(&v).unwrap(),
                format!("\"{}\"", word(v))
            );
        }
    }

    #[test]
    fn every_word_is_its_serde_name_and_reads_back() {
        same_as_serde(Case::ALL, Case::word);
        same_as_serde(CensorStyle::ALL, CensorStyle::word);
        same_as_serde(Animation::ALL, Animation::word);
        same_as_serde(ExistingClips::ALL, ExistingClips::word);
        same_as_serde(Layout::ALL, Layout::word);
        for &a in Animation::ALL {
            assert_eq!(Animation::from_word(a.word()), Some(a));
        }
        for &l in Layout::ALL {
            assert_eq!(Layout::from_word(l.word()), Some(l));
        }
        for &e in ExistingClips::ALL {
            assert_eq!(ExistingClips::from_word(e.word()), Some(e));
        }
        assert_eq!(Layout::from_word("Side"), None);
    }
}
