use std::ops::Range;

use gfx_hal::{Backend, MemoryTypeId};
use gfx_hal::memory::Requirements;

use {MemoryAllocator, MemoryError, MemorySubAllocator};
use arena::{ArenaAllocator, ArenaBlock};
use block::{Block, RawBlock};
use chunked::{ChunkedAllocator, ChunkedBlock};
use root::RootAllocator;

/// Controls what sub allocator is used for an allocation by `CombinedAllocator`
#[derive(Clone, Copy, Debug)]
pub enum Type {
    /// For short-lived objects, such as staging buffers.
    ShortLived,

    /// General purpose.
    General,
}

/// Combines `ArenaAllocator` and `ChunkedAllocator`, and allows the user to control which type of
/// allocation to use.
///
/// Use `RootAllocator` as the super allocator, which will handle the actual memory allocations
/// from `Device`.
///
/// ### Type parameters:
///
/// - `B`: hal `Backend`
#[derive(Debug)]
pub struct CombinedAllocator<B>
where
    B: Backend,
{
    root: RootAllocator<B>,
    arenas: ArenaAllocator<RawBlock<B>>,
    chunks: ChunkedAllocator<RawBlock<B>>,
}

impl<B> CombinedAllocator<B>
where
    B: Backend,
{
    /// Create a combined allocator.
    ///
    /// ### Parameters:
    ///
    /// - `memory_type_id`: hal memory type
    /// - `arena_size`: see `ArenaAllocator`
    /// - `chunks_per_block`: see `ChunkedAllocator`
    /// - `min_chunk_size`: see `ChunkedAllocator`
    /// - `max_chunk_size`: see `ChunkedAllocator`
    pub fn new(
        memory_type_id: MemoryTypeId,
        arena_size: u64,
        chunks_per_block: usize,
        min_chunk_size: u64,
        max_chunk_size: u64,
    ) -> Self {
        CombinedAllocator {
            root: RootAllocator::new(memory_type_id),
            arenas: ArenaAllocator::new(arena_size, memory_type_id),
            chunks: ChunkedAllocator::new(
                chunks_per_block,
                min_chunk_size,
                max_chunk_size,
                memory_type_id,
            ),
        }
    }

    /// Get memory type id
    pub fn memory_type(&self) -> MemoryTypeId {
        self.root.memory_type()
    }
}

impl<B> MemoryAllocator<B> for CombinedAllocator<B>
where
    B: Backend,
{
    type Request = Type;
    type Block = CombinedBlock<B>;

    fn alloc(
        &mut self,
        device: &B::Device,
        request: Type,
        reqs: Requirements,
    ) -> Result<CombinedBlock<B>, MemoryError> {
        match request {
            Type::ShortLived => {
                self.arenas
                    .alloc(&mut self.root, device, (), reqs)
                    .map(|ArenaBlock(block, tag)| CombinedBlock(block, CombinedTag::Arena(tag)))
            }
            Type::General => {
                if reqs.size > self.chunks.max_chunk_size() {
                    self.root
                        .alloc(device, (), reqs)
                        .map(|block| CombinedBlock(block, CombinedTag::Root))
                } else {
                    self.chunks
                        .alloc(&mut self.root, device, (), reqs)
                        .map(|ChunkedBlock(block, tag)| CombinedBlock(block, CombinedTag::Chunked(tag)))
                }
            }
        }
    }

    fn free(&mut self, device: &B::Device, block: CombinedBlock<B>) {
        match block.1 {
            CombinedTag::Arena(tag) => self.arenas.free(&mut self.root, device, ArenaBlock(block.0, tag)),
            CombinedTag::Chunked(tag) => self.chunks.free(&mut self.root, device, ChunkedBlock(block.0, tag)),
            CombinedTag::Root => self.root.free(device, block.0),
        }
    }

    fn is_used(&self) -> bool {
        let used = self.arenas.is_used() || self.chunks.is_used();
        assert_eq!(used, self.root.is_used());
        used
    }

    fn dispose(mut self, device: &B::Device) -> Result<(), Self> {
        let memory_type_id = self.root.memory_type();
        let arena_size = self.arenas.arena_size();
        let chunks_per_block = self.chunks.chunks_per_block();
        let min_chunk_size = self.chunks.min_chunk_size();
        let max_chunk_size = self.chunks.max_chunk_size();

        let arenas = self.arenas.dispose(&mut self.root, device);
        let chunks = self.chunks.dispose(&mut self.root, device);

        if arenas.is_err() || chunks.is_err() {
            let arenas = arenas
                .err()
                .unwrap_or_else(|| ArenaAllocator::new(arena_size, memory_type_id));
            let chunks = chunks.err().unwrap_or_else(|| {
                ChunkedAllocator::new(
                    chunks_per_block,
                    min_chunk_size,
                    max_chunk_size,
                    memory_type_id,
                )
            });

            Err(CombinedAllocator {
                root: self.root,
                arenas,
                chunks,
            })
        } else {
            self.root.dispose(device).unwrap();
            Ok(())
        }
    }
}

/// Opaque type for `Block` tag used by the `CombinedAllocator`.
///
/// `CombinedAllocator` places this tag on the memory blocks, and then use it in
/// `free` to find the memory node the block was allocated from.
#[derive(Debug)]
pub struct CombinedBlock<B: Backend>(pub(crate) RawBlock<B>, pub(crate) CombinedTag);

#[derive(Debug)]
pub(crate) enum CombinedTag {
    Arena(u64),
    Chunked(usize),
    Root,
}

impl<B> Block<B> for CombinedBlock<B>
where
    B: Backend,
{
    /// Get memory of the block.
    #[inline(always)]
    fn memory(&self) -> &B::Memory {
        // Has to be valid
        self.0.memory()
    }

    /// Get memory range of the block.
    #[inline(always)]
    fn range(&self) -> Range<u64> {
        self.0.range()
    }
}
