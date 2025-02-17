//! `AstIdMap` allows to create stable IDs for "large" syntax nodes like items
//! and macro calls.
//!
//! Specifically, it enumerates all items in a file and uses position of a an
//! item as an ID. That way, id's don't change unless the set of items itself
//! changes.

// FIXME: Consider moving this into the span crate

use std::{
    any::type_name,
    fmt,
    hash::{BuildHasher, BuildHasherDefault, Hash, Hasher},
    marker::PhantomData,
};

use la_arena::{Arena, Idx, RawIdx};
use profile::Count;
use rustc_hash::FxHasher;
use syntax::{ast, AstNode, AstPtr, SyntaxNode, SyntaxNodePtr};

use crate::db::ExpandDatabase;

pub use span::ErasedFileAstId;

/// `AstId` points to an AST node in any file.
///
/// It is stable across reparses, and can be used as salsa key/value.
pub type AstId<N> = crate::InFile<FileAstId<N>>;

impl<N: AstIdNode> AstId<N> {
    pub fn to_node(&self, db: &dyn ExpandDatabase) -> N {
        self.to_ptr(db).to_node(&db.parse_or_expand(self.file_id))
    }
    pub fn to_in_file_node(&self, db: &dyn ExpandDatabase) -> crate::InFile<N> {
        crate::InFile::new(self.file_id, self.to_ptr(db).to_node(&db.parse_or_expand(self.file_id)))
    }
    pub fn to_ptr(&self, db: &dyn ExpandDatabase) -> AstPtr<N> {
        db.ast_id_map(self.file_id).get(self.value)
    }
}

pub type ErasedAstId = crate::InFile<ErasedFileAstId>;

impl ErasedAstId {
    pub fn to_ptr(&self, db: &dyn ExpandDatabase) -> SyntaxNodePtr {
        db.ast_id_map(self.file_id).get_erased(self.value)
    }
}

/// `AstId` points to an AST node in a specific file.
pub struct FileAstId<N: AstIdNode> {
    raw: ErasedFileAstId,
    covariant: PhantomData<fn() -> N>,
}

impl<N: AstIdNode> Clone for FileAstId<N> {
    fn clone(&self) -> FileAstId<N> {
        *self
    }
}
impl<N: AstIdNode> Copy for FileAstId<N> {}

impl<N: AstIdNode> PartialEq for FileAstId<N> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}
impl<N: AstIdNode> Eq for FileAstId<N> {}
impl<N: AstIdNode> Hash for FileAstId<N> {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.raw.hash(hasher);
    }
}

impl<N: AstIdNode> fmt::Debug for FileAstId<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FileAstId::<{}>({})", type_name::<N>(), self.raw.into_raw())
    }
}

impl<N: AstIdNode> FileAstId<N> {
    // Can't make this a From implementation because of coherence
    pub fn upcast<M: AstIdNode>(self) -> FileAstId<M>
    where
        N: Into<M>,
    {
        FileAstId { raw: self.raw, covariant: PhantomData }
    }

    pub fn erase(self) -> ErasedFileAstId {
        self.raw
    }
}

pub trait AstIdNode: AstNode {}
macro_rules! register_ast_id_node {
    (impl AstIdNode for $($ident:ident),+ ) => {
        $(
            impl AstIdNode for ast::$ident {}
        )+
        fn should_alloc_id(kind: syntax::SyntaxKind) -> bool {
            $(
                ast::$ident::can_cast(kind)
            )||+
        }
    };
}
register_ast_id_node! {
    impl AstIdNode for
    Item,
        Adt,
            Enum,
            Struct,
            Union,
        Const,
        ExternBlock,
        ExternCrate,
        Fn,
        Impl,
        Macro,
            MacroDef,
            MacroRules,
        MacroCall,
        Module,
        Static,
        Trait,
        TraitAlias,
        TypeAlias,
        Use,
    AssocItem, BlockExpr, Variant, RecordField, TupleField, ConstArg, Param, SelfParam
}

/// Maps items' `SyntaxNode`s to `ErasedFileAstId`s and back.
#[derive(Default)]
pub struct AstIdMap {
    /// Maps stable id to unstable ptr.
    arena: Arena<SyntaxNodePtr>,
    /// Reverse: map ptr to id.
    map: hashbrown::HashMap<Idx<SyntaxNodePtr>, (), ()>,
    _c: Count<Self>,
}

impl fmt::Debug for AstIdMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AstIdMap").field("arena", &self.arena).finish()
    }
}

impl PartialEq for AstIdMap {
    fn eq(&self, other: &Self) -> bool {
        self.arena == other.arena
    }
}
impl Eq for AstIdMap {}

impl AstIdMap {
    pub(crate) fn from_source(node: &SyntaxNode) -> AstIdMap {
        assert!(node.parent().is_none());
        let mut res = AstIdMap::default();

        // make sure to allocate the root node
        if !should_alloc_id(node.kind()) {
            res.alloc(node);
        }
        // By walking the tree in breadth-first order we make sure that parents
        // get lower ids then children. That is, adding a new child does not
        // change parent's id. This means that, say, adding a new function to a
        // trait does not change ids of top-level items, which helps caching.
        bdfs(node, |it| {
            if should_alloc_id(it.kind()) {
                res.alloc(&it);
                TreeOrder::BreadthFirst
            } else {
                TreeOrder::DepthFirst
            }
        });
        res.map = hashbrown::HashMap::with_capacity_and_hasher(res.arena.len(), ());
        for (idx, ptr) in res.arena.iter() {
            let hash = hash_ptr(ptr);
            match res.map.raw_entry_mut().from_hash(hash, |idx2| *idx2 == idx) {
                hashbrown::hash_map::RawEntryMut::Occupied(_) => unreachable!(),
                hashbrown::hash_map::RawEntryMut::Vacant(entry) => {
                    entry.insert_with_hasher(hash, idx, (), |&idx| hash_ptr(&res.arena[idx]));
                }
            }
        }
        res.arena.shrink_to_fit();
        res
    }

    /// The [`AstId`] of the root node
    pub fn root(&self) -> SyntaxNodePtr {
        self.arena[Idx::from_raw(RawIdx::from_u32(0))].clone()
    }

    pub fn ast_id<N: AstIdNode>(&self, item: &N) -> FileAstId<N> {
        let raw = self.erased_ast_id(item.syntax());
        FileAstId { raw, covariant: PhantomData }
    }

    pub fn get<N: AstIdNode>(&self, id: FileAstId<N>) -> AstPtr<N> {
        AstPtr::try_from_raw(self.arena[id.raw].clone()).unwrap()
    }

    pub fn get_erased(&self, id: ErasedFileAstId) -> SyntaxNodePtr {
        self.arena[id].clone()
    }

    fn erased_ast_id(&self, item: &SyntaxNode) -> ErasedFileAstId {
        let ptr = SyntaxNodePtr::new(item);
        let hash = hash_ptr(&ptr);
        match self.map.raw_entry().from_hash(hash, |&idx| self.arena[idx] == ptr) {
            Some((&idx, &())) => idx,
            None => panic!(
                "Can't find {:?} in AstIdMap:\n{:?}",
                item,
                self.arena.iter().map(|(_id, i)| i).collect::<Vec<_>>(),
            ),
        }
    }

    fn alloc(&mut self, item: &SyntaxNode) -> ErasedFileAstId {
        self.arena.alloc(SyntaxNodePtr::new(item))
    }
}

fn hash_ptr(ptr: &SyntaxNodePtr) -> u64 {
    let mut hasher = BuildHasherDefault::<FxHasher>::default().build_hasher();
    ptr.hash(&mut hasher);
    hasher.finish()
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum TreeOrder {
    BreadthFirst,
    DepthFirst,
}

/// Walks the subtree in bdfs order, calling `f` for each node. What is bdfs
/// order? It is a mix of breadth-first and depth first orders. Nodes for which
/// `f` returns [`TreeOrder::BreadthFirst`] are visited breadth-first, all the other nodes are explored
/// [`TreeOrder::DepthFirst`].
///
/// In other words, the size of the bfs queue is bound by the number of "true"
/// nodes.
fn bdfs(node: &SyntaxNode, mut f: impl FnMut(SyntaxNode) -> TreeOrder) {
    let mut curr_layer = vec![node.clone()];
    let mut next_layer = vec![];
    while !curr_layer.is_empty() {
        curr_layer.drain(..).for_each(|node| {
            let mut preorder = node.preorder();
            while let Some(event) = preorder.next() {
                match event {
                    syntax::WalkEvent::Enter(node) => {
                        if f(node.clone()) == TreeOrder::BreadthFirst {
                            next_layer.extend(node.children());
                            preorder.skip_subtree();
                        }
                    }
                    syntax::WalkEvent::Leave(_) => {}
                }
            }
        });
        std::mem::swap(&mut curr_layer, &mut next_layer);
    }
}
