//! File and span related types.
// FIXME: This should be moved into its own crate to get rid of the dependency inversion, base-db
// has business depending on tt, tt should depend on a span crate only (which unforunately will have
// to depend on salsa)
use std::fmt;

use salsa::InternId;

mod map;

pub use crate::map::{RealSpanMap, SpanMap};
pub use syntax::{TextRange, TextSize};
pub use vfs::FileId;

#[derive(Clone, Copy, Debug)]
pub struct FilePosition {
    pub file_id: FileId,
    pub offset: TextSize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct FileRange {
    pub file_id: FileId,
    pub range: TextRange,
}

pub type ErasedFileAstId = la_arena::Idx<syntax::SyntaxNodePtr>;

// The first inde is always the root node's AstId
pub const ROOT_ERASED_FILE_AST_ID: ErasedFileAstId =
    la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(0));

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SpanData<Ctx> {
    /// The text range of this span, relative to the anchor.
    /// We need the anchor for incrementality, as storing absolute ranges will require
    /// recomputation on every change in a file at all times.
    pub range: TextRange,
    pub anchor: SpanAnchor,
    /// The syntax context of the span.
    pub ctx: Ctx,
}
impl Span {
    #[deprecated = "dummy spans will panic if surfaced incorrectly, as such they should be replaced appropriately"]
    pub const DUMMY: Self = SpanData {
        range: TextRange::empty(TextSize::new(0)),
        anchor: SpanAnchor { file_id: FileId::BOGUS, ast_id: ROOT_ERASED_FILE_AST_ID },
        ctx: SyntaxContextId::ROOT,
    };
}

pub type Span = SpanData<SyntaxContextId>;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyntaxContextId(InternId);

impl fmt::Debug for SyntaxContextId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Self::SELF_REF {
            f.debug_tuple("SyntaxContextId")
                .field(&{
                    #[derive(Debug)]
                    #[allow(non_camel_case_types)]
                    struct SELF_REF;
                    SELF_REF
                })
                .finish()
        } else {
            f.debug_tuple("SyntaxContextId").field(&self.0).finish()
        }
    }
}

impl salsa::InternKey for SyntaxContextId {
    fn from_intern_id(v: salsa::InternId) -> Self {
        SyntaxContextId(v)
    }
    fn as_intern_id(&self) -> salsa::InternId {
        self.0
    }
}

impl fmt::Display for SyntaxContextId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_u32())
    }
}

// inherent trait impls please tyvm
impl SyntaxContextId {
    pub const ROOT: Self = SyntaxContextId(unsafe { InternId::new_unchecked(0) });
    // veykril(HACK): FIXME salsa doesn't allow us fetching the id of the current input to be allocated so
    // we need a special value that behaves as the current context.
    pub const SELF_REF: Self =
        SyntaxContextId(unsafe { InternId::new_unchecked(InternId::MAX - 1) });

    pub fn is_root(self) -> bool {
        self == Self::ROOT
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct SpanAnchor {
    pub file_id: FileId,
    pub ast_id: ErasedFileAstId,
}

impl fmt::Debug for SpanAnchor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SpanAnchor").field(&self.file_id).field(&self.ast_id.into_raw()).finish()
    }
}

/// Input to the analyzer is a set of files, where each file is identified by
/// `FileId` and contains source code. However, another source of source code in
/// Rust are macros: each macro can be thought of as producing a "temporary
/// file". To assign an id to such a file, we use the id of the macro call that
/// produced the file. So, a `HirFileId` is either a `FileId` (source code
/// written by user), or a `MacroCallId` (source code produced by macro).
///
/// What is a `MacroCallId`? Simplifying, it's a `HirFileId` of a file
/// containing the call plus the offset of the macro call in the file. Note that
/// this is a recursive definition! However, the size_of of `HirFileId` is
/// finite (because everything bottoms out at the real `FileId`) and small
/// (`MacroCallId` uses the location interning. You can check details here:
/// <https://en.wikipedia.org/wiki/String_interning>).
///
/// The two variants are encoded in a single u32 which are differentiated by the MSB.
/// If the MSB is 0, the value represents a `FileId`, otherwise the remaining 31 bits represent a
/// `MacroCallId`.
// FIXME: Give this a better fitting name
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HirFileId(u32);

impl From<HirFileId> for u32 {
    fn from(value: HirFileId) -> Self {
        value.0
    }
}

impl From<MacroCallId> for HirFileId {
    fn from(value: MacroCallId) -> Self {
        value.as_file()
    }
}

impl fmt::Debug for HirFileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.repr().fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacroFileId {
    pub macro_call_id: MacroCallId,
}

/// `MacroCallId` identifies a particular macro invocation, like
/// `println!("Hello, {}", world)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MacroCallId(salsa::InternId);

impl salsa::InternKey for MacroCallId {
    fn from_intern_id(v: salsa::InternId) -> Self {
        MacroCallId(v)
    }
    fn as_intern_id(&self) -> salsa::InternId {
        self.0
    }
}

impl MacroCallId {
    pub fn as_file(self) -> HirFileId {
        MacroFileId { macro_call_id: self }.into()
    }

    pub fn as_macro_file(self) -> MacroFileId {
        MacroFileId { macro_call_id: self }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum HirFileIdRepr {
    FileId(FileId),
    MacroFile(MacroFileId),
}

impl fmt::Debug for HirFileIdRepr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileId(arg0) => f.debug_tuple("FileId").field(&arg0.index()).finish(),
            Self::MacroFile(arg0) => {
                f.debug_tuple("MacroFile").field(&arg0.macro_call_id.0).finish()
            }
        }
    }
}

impl From<FileId> for HirFileId {
    fn from(id: FileId) -> Self {
        _ = Self::ASSERT_MAX_FILE_ID_IS_SAME;
        assert!(id.index() <= Self::MAX_HIR_FILE_ID, "FileId index {} is too large", id.index());
        HirFileId(id.index())
    }
}

impl From<MacroFileId> for HirFileId {
    fn from(MacroFileId { macro_call_id: MacroCallId(id) }: MacroFileId) -> Self {
        _ = Self::ASSERT_MAX_FILE_ID_IS_SAME;
        let id = id.as_u32();
        assert!(id <= Self::MAX_HIR_FILE_ID, "MacroCallId index {} is too large", id);
        HirFileId(id | Self::MACRO_FILE_TAG_MASK)
    }
}

impl HirFileId {
    const ASSERT_MAX_FILE_ID_IS_SAME: () =
        [()][(Self::MAX_HIR_FILE_ID != FileId::MAX_FILE_ID) as usize];

    const MAX_HIR_FILE_ID: u32 = u32::MAX ^ Self::MACRO_FILE_TAG_MASK;
    const MACRO_FILE_TAG_MASK: u32 = 1 << 31;

    #[inline]
    pub fn is_macro(self) -> bool {
        self.0 & Self::MACRO_FILE_TAG_MASK != 0
    }

    #[inline]
    pub fn macro_file(self) -> Option<MacroFileId> {
        match self.0 & Self::MACRO_FILE_TAG_MASK {
            0 => None,
            _ => Some(MacroFileId {
                macro_call_id: MacroCallId(InternId::from(self.0 ^ Self::MACRO_FILE_TAG_MASK)),
            }),
        }
    }

    #[inline]
    pub fn file_id(self) -> Option<FileId> {
        match self.0 & Self::MACRO_FILE_TAG_MASK {
            0 => Some(FileId::from_raw(self.0)),
            _ => None,
        }
    }

    #[inline]
    pub fn repr(self) -> HirFileIdRepr {
        match self.0 & Self::MACRO_FILE_TAG_MASK {
            0 => HirFileIdRepr::FileId(FileId::from_raw(self.0)),
            _ => HirFileIdRepr::MacroFile(MacroFileId {
                macro_call_id: MacroCallId(InternId::from(self.0 ^ Self::MACRO_FILE_TAG_MASK)),
            }),
        }
    }
}
